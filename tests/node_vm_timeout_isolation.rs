//! 验证同一 Engine 上一个 Store 的 vm timeout 不会中断另一个 Store。
//!
//! 断言只看可观察行为（exit code / stdout 内容），不依赖墙钟上限。
//! A 的 timeout 语义由 vm 的 deadline 保证；B 必须完整算完，证明 epoch 不跨 Store 误杀。

use std::fs;
use std::path::PathBuf;
use std::thread;

fn write_temp_script(name: &str, source: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wjsm-timeout-isolation-{}-{}",
        std::process::id(),
        name
    ));
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("main.js");
    fs::write(&path, source).expect("write temp script");
    path
}

#[test]
fn node_vm_timeout_does_not_interrupt_other_store() {
    let path_a = write_temp_script(
        "a",
        r#"
const vm = require('vm');
var timedOut = false;
try {
  vm.runInNewContext('while(true){}', {}, { timeout: 50 });
} catch (e) {
  var s = String(e && e.message || e);
  timedOut = s.indexOf('timed out') >= 0;
}
console.log('A:' + timedOut);
"#,
    );
    let path_b = write_temp_script(
        "b",
        r#"
let s = 0;
for (let i = 0; i < 10000; i++) s += i;
console.log('B:' + s);
"#,
    );

    // 并发启动：不 sleep 协调时序；正确性只要求最终结果。
    let handle_a = {
        let p = path_a.clone();
        thread::spawn(move || {
            wjsm_cli::run_file_in_process_with_options(
                &p,
                &[],
                &[("WJSM_COMPILER", "winch")],
                None,
            )
        })
    };
    let handle_b = {
        let p = path_b.clone();
        thread::spawn(move || {
            wjsm_cli::run_file_in_process_with_options(
                &p,
                &[],
                &[("WJSM_COMPILER", "winch")],
                None,
            )
        })
    };

    let (code_a, out_a, err_a) = handle_a.join().expect("join A");
    let (code_b, out_b, err_b) = handle_b.join().expect("join B");
    let out_a = String::from_utf8_lossy(&out_a);
    let out_b = String::from_utf8_lossy(&out_b);
    let err_a = String::from_utf8_lossy(&err_a);
    let err_b = String::from_utf8_lossy(&err_b);

    assert_eq!(code_a, 0, "A exit: stdout={out_a} stderr={err_a}");
    assert_eq!(code_b, 0, "B exit: stdout={out_b} stderr={err_b}");
    assert!(
        out_a.contains("A:true"),
        "store A should timeout: {out_a:?} err={err_a:?}"
    );
    assert!(
        out_b.contains("B:49995000"),
        "store B must complete despite A timeout: {out_b:?} err={err_b:?}"
    );

    let _ = fs::remove_dir_all(path_a.parent().unwrap());
    let _ = fs::remove_dir_all(path_b.parent().unwrap());
}
