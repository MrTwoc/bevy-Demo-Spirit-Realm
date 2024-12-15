use std::collections::HashMap;

/*
    测试：
    std库的hash函数的性能
    测试环境：
    CPU: I7 8700K
    gpu: GTX 1060
    内存: DDR4 2x8G
    操作系统: windows10 x64

    测试结果：
    warning: unused manifest key: examples
    Compiling bevygame1 v0.1.0 (J:\MyGames\bevygame1)
    Finished `dev` profile [optimized + debuginfo] target(s) in 0.76s
     Running `target\debug\examples\stdhash.exe`
    test round 95, sum:499999500000, elapsed:62.4095ms
    test round 96, sum:499999500000, elapsed:66.2427ms
    test round 97, sum:499999500000, elapsed:67.9611ms
    test round 98, sum:499999500000, elapsed:65.8215ms
    test round 99, sum:499999500000, elapsed:63.894ms
*/

fn main(){
    let hash:HashMap<i64, i64> = (0..1_000_000).map(|i|(i,i)).collect();

    for t in 0..100{
        let mut total = 0;
        let start = std::time::Instant::now();
        for k in 0..1_000_000{
            let value = hash.get(&k).unwrap();
            total += value;
        }
        let elapsed = std::time::Instant::now().saturating_duration_since(start);

        if t >= 95 {
            println!("test round {}, sum:{}, elapsed:{:?}",t, total, elapsed);
        }

    }
}