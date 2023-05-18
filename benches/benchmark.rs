use criterion::{criterion_group, criterion_main, Criterion};
use smithay::utils::user_data::UserDataMap;

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("UserDataMap::get", |b| {
        let udata_map = UserDataMap::new();
        udata_map.insert_if_missing(|| 17i32);
        b.iter(|| udata_map.get::<i32>())
    });
    c.bench_function("UserDataMap::get threadsafe", |b| {
        let udata_map = UserDataMap::new();
        udata_map.insert_if_missing_threadsafe(|| 17i32);
        b.iter(|| udata_map.get::<i32>())
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
