use criterion::{black_box, criterion_group, criterion_main, Criterion};
use entropy_randomness::cpu::{detect_cpu_safely, get_cpu_temperature, mask_cpu_identity, bench};

fn bench_cpu_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("CPU Detection");
    
    group.bench_function("detect_cpu_safely", |b| {
        b.iter(|| {
            // Allow cache to be used - real-world scenario
            let cpu = black_box(detect_cpu_safely());
            black_box(cpu.vendor());
        });
    });
    
    group.bench_function("detect_cpu_cold_start", |b| {
        b.iter(|| {
            // Force re-detection each time
            let elapsed = black_box(bench::bench_cpu_detection());
            black_box(elapsed);
        });
    });
    
    group.bench_function("cpu_fingerprinting", |b| {
        let cpu = detect_cpu_safely();
        b.iter(|| {
            let fingerprint = black_box(cpu.telemetry_fingerprint());
            black_box(fingerprint);
        });
    });
    
    group.bench_function("cpu_masking_50pct", |b| {
        b.iter(|| {
            let masked = black_box(mask_cpu_identity(None, Some(50.0)));
            black_box(masked.vendor());
        });
    });
    
    group.finish();
}

fn bench_temperature(c: &mut Criterion) {
    let mut group = c.benchmark_group("Temperature");
    
    group.bench_function("get_cpu_temperature", |b| {
        b.iter(|| {
            let temp = black_box(get_cpu_temperature());
            black_box(temp);
        });
    });
    
    group.bench_function("get_cpu_temperature_uncached", |b| {
        b.iter(|| {
            let elapsed = black_box(bench::bench_temperature_reading());
            black_box(elapsed);
        });
    });
    
    group.finish();
}

criterion_group!(benches, bench_cpu_detection, bench_temperature);
criterion_main!(benches);
