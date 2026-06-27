//! GC 持续分配压力 benchmark。
//!
//! 连续分配远超 GC 阈值（1000）的临时对象，跑数百轮 GC 周期，度量长时间分配
//! churn 下的吞吐，并验证不 OOM、存活对象不被损坏。这是性能/压力验证，不属于
//! 正确性测试套件——跨 GC 存活的正确性由 fixtures/happy/gc_*.js 的快速 fixture 覆盖。
//!
//! 运行：cargo bench -p wjsm-runtime --bench gc_stress

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use wjsm_runtime::{compile_source, execute_with_writer};

fn gc_sustained_allocation(c: &mut Criterion) {
    // 编译只做一次：被度量的是 GC churn 下的执行，而非编译。
    let wasm = compile_source(
        r#"
        let total = 0;
        for (let i = 0; i < 200000; i++) {
          const tmp = { x: i, y: i + 1 };
          total += tmp.x;
        }
        "#,
    )
    .expect("compile gc stress source");
    let rt = Runtime::new().expect("tokio runtime");

    let mut group = c.benchmark_group("gc");
    // 单次 execute 较重（数百轮 GC），缩小采样数避免 bench 过久。
    group.sample_size(10);
    group.bench_function("sustained_allocation", |b| {
        b.iter(|| {
            let out = rt
                .block_on(async { execute_with_writer(&wasm, Vec::new()).await })
                .expect("execute gc stress");
            black_box(out);
        });
    });
    group.finish();
}

criterion_group!(benches, gc_sustained_allocation);
criterion_main!(benches);
