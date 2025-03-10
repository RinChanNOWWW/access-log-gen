use arrow::array::{
    Int32Builder, StringBuilder, StringDictionaryBuilder, TimestampMicrosecondBuilder,
    TimestampNanosecondBuilder, UInt16Builder,
};
use arrow::datatypes::{DataType, Field, Int32Type, Schema, SchemaRef, TimeUnit};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterPropertiesBuilder};
use parquet::file::reader::SerializedPageReader;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::fs::File;
use std::ops::Range;
use std::sync::Arc;

#[derive(Default)]
struct BatchBuilder {
    service: StringBuilder,
    host: StringBuilder,
    pod: StringBuilder,
    container: StringBuilder,
    image: StringBuilder,
    time: TimestampMicrosecondBuilder,
    client_addr: StringBuilder,
    request_duration: Int32Builder,
    request_user_agent: StringBuilder,
    request_method: StringBuilder,
    request_host: StringBuilder,
    request_bytes: Int32Builder,
    response_bytes: Int32Builder,
    response_status: UInt16Builder,
}

impl BatchBuilder {
    fn schema() -> SchemaRef {
        // let utf8_dict =
        //     || DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));

        Arc::new(Schema::new(vec![
            Field::new("service", DataType::Utf8, true),
            Field::new("host", DataType::Utf8, false),
            Field::new("pod", DataType::Utf8, false),
            Field::new("container", DataType::Utf8, false),
            Field::new("image", DataType::Utf8, false),
            Field::new(
                "time",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                false,
            ),
            Field::new("client_addr", DataType::Utf8, true),
            Field::new("request_duration_ns", DataType::Int32, false),
            Field::new("request_user_agent", DataType::Utf8, true),
            Field::new("request_method", DataType::Utf8, true),
            Field::new("request_host", DataType::Utf8, true),
            Field::new("request_bytes", DataType::Int32, true),
            Field::new("response_bytes", DataType::Int32, true),
            Field::new("response_status", DataType::UInt16, false),
        ]))
    }

    fn append(&mut self, rng: &mut StdRng, host: &str, service: &str) {
        let num_pods = rng.gen_range(1..15);
        let pods = generate_sorted_strings(rng, num_pods, 30..40);
        for pod in pods {
            for container_idx in 0..rng.gen_range(1..3) {
                let container = format!("{}_container_{}", service, container_idx);
                let image = format!(
                    "{}@sha256:30375999bf03beec2187843017b10c9e88d8b1a91615df4eb6350fb39472edd9",
                    container
                );

                let num_entries = rng.gen_range(1024..8192);
                for i in 0..num_entries {
                    let time = i as i64 * 1024;
                    self.append_row(rng, host, &pod, service, &container, &image, time);
                }
            }
        }
    }

    fn append_row(
        &mut self,
        rng: &mut StdRng,
        host: &str,
        pod: &str,
        service: &str,
        container: &str,
        image: &str,
        time: i64,
    ) {
        let methods = &["GET", "PUT", "POST", "HEAD", "PATCH", "DELETE"];
        let status = &[200, 204, 400, 503, 403];

        self.service.append_value(service);
        self.host.append_value(host);
        self.pod.append_value(pod);
        self.container.append_value(container);
        self.image.append_value(image);
        self.time.append_value(time);

        self.client_addr.append_value(format!(
            "{}.{}.{}.{}",
            rng.gen::<u8>(),
            rng.gen::<u8>(),
            rng.gen::<u8>(),
            rng.gen::<u8>()
        ));
        self.request_duration.append_value(rng.gen());
        self.request_user_agent
            .append_value(random_string(rng, 20..100));
        self.request_method
            .append_value(methods[rng.gen_range(0..methods.len())]);
        self.request_host
            .append_value(format!("https://{}.mydomain.com", service));

        self.request_bytes
            .append_option(rng.gen_bool(0.9).then(|| rng.gen()));
        self.response_bytes
            .append_option(rng.gen_bool(0.9).then(|| rng.gen()));
        self.response_status
            .append_value(status[rng.gen_range(0..status.len())]);
    }

    fn finish(mut self, schema: SchemaRef) -> RecordBatch {
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(self.service.finish()),
                Arc::new(self.host.finish()),
                Arc::new(self.pod.finish()),
                Arc::new(self.container.finish()),
                Arc::new(self.image.finish()),
                Arc::new(self.time.finish()),
                Arc::new(self.client_addr.finish()),
                Arc::new(self.request_duration.finish()),
                Arc::new(self.request_user_agent.finish()),
                Arc::new(self.request_method.finish()),
                Arc::new(self.request_host.finish()),
                Arc::new(self.request_bytes.finish()),
                Arc::new(self.response_bytes.finish()),
                Arc::new(self.response_status.finish()),
            ],
        )
        .unwrap()
    }
}

fn random_string(rng: &mut StdRng, len_range: Range<usize>) -> String {
    let len = rng.gen_range(len_range);
    (0..len)
        .map(|_| rng.gen_range(b'a'..b'z') as char)
        .collect::<String>()
}

fn generate_sorted_strings(rng: &mut StdRng, count: usize, str_len: Range<usize>) -> Vec<String> {
    let mut strings: Vec<_> = (0..count)
        .map(|_| random_string(rng, str_len.clone()))
        .collect();

    strings.sort_unstable();
    strings
}

/// Generates sorted RecordBatch with an access log style schema for a single host
#[derive(Debug)]
struct Generator {
    schema: SchemaRef,
    rng: StdRng,
    host_idx: usize,
}

impl Generator {
    fn new() -> Self {
        let seed = [
            1, 0, 0, 0, 23, 0, 3, 0, 200, 1, 0, 0, 210, 30, 8, 0, 1, 0, 21, 0, 6, 0, 0, 0, 0, 0, 5,
            0, 0, 0, 0, 0,
        ];

        Self {
            schema: BatchBuilder::schema(),
            host_idx: 0,
            rng: StdRng::from_seed(seed),
        }
    }
}

impl Iterator for Generator {
    type Item = RecordBatch;

    fn next(&mut self) -> Option<Self::Item> {
        let mut builder = BatchBuilder::default();

        let host = format!(
            "i-{:016x}.ec2.internal",
            self.host_idx * 0x7d87f8ed5c5 + 0x1ec3ca3151468928
        );
        self.host_idx += 1;

        for service in &["frontend", "backend", "database", "cache"] {
            if self.rng.gen_bool(0.5) {
                continue;
            }
            builder.append(&mut self.rng, &host, service);
        }
        Some(builder.finish(Arc::clone(&self.schema)))
    }
}

fn write_parquet(
    name: &str,
    schema: SchemaRef,
    batches: &[RecordBatch],
    write_props: WriterProperties,
) {
    let mut file = File::create(name).unwrap();
    let mut writer = ArrowWriter::try_new(&mut file, schema, Some(write_props)).unwrap();
    for batch in batches {
        writer.write(&batch).unwrap();
    }
    writer.close().unwrap();
}

fn main() {
    let generator = Generator::new();
    let schema = generator.schema.clone();
    let batches = generator.take(40).collect::<Vec<_>>();

    write_parquet(
        "logs-no-stats.parquet",
        schema.clone(),
        &batches,
        WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build(),
    );
    println!("Write logs-no-stats.parquet");

    write_parquet(
        "logs-chunk-stats.parquet",
        schema.clone(),
        &batches,
        WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::Chunk)
            .build(),
    );
    println!("Write logs-chunk-stats.parquet");

    write_parquet(
        "logs-page-stats.parquet",
        schema.clone(),
        &batches,
        WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::Page)
            .build(),
    );
    println!("Write logs-page-stats.parquet");

    // let file = File::open("logs.parquet").unwrap();

    // let options = ArrowReaderOptions::new().with_page_index(false);
    // let reader =
    //     ParquetRecordBatchReaderBuilder::try_new_with_options(file.try_clone().unwrap(), options)
    //         .unwrap();

    // let chunk_reader = Arc::new(file);
    // for (r_idx, row_group) in reader.metadata().row_groups().iter().enumerate() {
    //     for (c_idx, column) in row_group.columns().iter().enumerate() {
    //         let page_reader = SerializedPageReader::new(
    //             Arc::clone(&chunk_reader),
    //             column,
    //             row_group.num_rows() as usize,
    //             None,
    //         )
    //         .unwrap();
    //         for (p_idx, page) in page_reader.enumerate() {
    //             let p = page.unwrap();
    //             println!(
    //                 "{}:{}:{} Page({},{},{})",
    //                 r_idx,
    //                 c_idx,
    //                 p_idx,
    //                 p.page_type(),
    //                 p.encoding(),
    //                 p.buffer().len()
    //             );
    //         }
    //     }
    // }
}
