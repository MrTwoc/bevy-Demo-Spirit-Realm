use nohash::NoHashHasher;
use std::{collections::HashMap, hash::BuildHasherDefault, process::Command};

/*
    测试：
    nohash库的hash函数的性能
    测试环境：
    CPU: I7 8700K   3.6GHz
    gpu: GTX 1060
    内存: DDR4 2x8G 3200MHz
    操作系统: windows10 x64
    rust版本: rustc 1.83.0 (90b35a623 2024-11-26)

    cargo build --release
    测试结果：
    test round 95, sum:499999500000, elapsed:1.7971ms
    test round 96, sum:499999500000, elapsed:1.9379ms
    test round 97, sum:499999500000, elapsed:1.9436ms
    test round 98, sum:499999500000, elapsed:2.0994ms
    test round 99, sum:499999500000, elapsed:1.9633ms
*/

fn main() {
    // let hash:HashMap<i64, i64> = (0..1_000_000).map(|i|(i,i)).collect();
    let nohash:HashMap<i64, i64, BuildHasherDefault<NoHashHasher<i64>>> = (0..1_000_000).map(|i|(i,i)).collect();

    for t in 0..100{
        let mut total = 0;
        let start = std::time::Instant::now();
        for k in 0..1_000_000{
            let value = nohash.get(&k).unwrap();
            total += value;
        }
        let elapsed = std::time::Instant::now().saturating_duration_since(start);

        if t >= 95 {
            println!("test round {}, sum:{}, elapsed:{:?}",t, total, elapsed);
        }

    }
    let _ = Command::new("cmd.exe").arg("/c").arg("pause").status();
    
}