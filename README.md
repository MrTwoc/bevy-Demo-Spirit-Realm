## 介绍
使用Bevy引擎+Rust编程语言，实现的一个简单的3D体素沙盒类游戏。

## 技术栈
| 1. Bevy  [游戏引擎](https://bevyengine.org/) <br>
| 2. Rust [编程语言](https://www.rust-lang.org/zh-CN/) <br>
| 3. Quic 通信协议 <br>
| 4. 噪声 用于生成地形，例如：<br>
OpenSimplex2 与 FBM 噪声：程序化生成地形 <br>
参考地址：
1. https://lib.rs/crates/noise
2. https://iquilezles.org/articles/morenoise/


## 核心功能 / 计划实现功能
- [ ] 方块放置/破坏<br>
- [ ] 各种优化剔除：  <br>
	遮挡剔除、视锥剔除、LOD / HLOD 技术、八叉树算法等<br>
- [ ] 方块材质加载<br>
- [ ] 区块系统实现<br>
- [ ] QUIC协议的应用<br>
- [ ] 服务端/多人游玩<br>
- [ ] AABB 碰撞箱<br>
- [ ] 玩家模型<br>
	待选模型文件格式：GLTF、JSON以及其他格式<br>

