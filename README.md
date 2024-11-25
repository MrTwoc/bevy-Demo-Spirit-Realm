## 介绍
使用Bevy引擎+Rust编程语言，实现的一个简单的3D体素沙盒类游戏。<br>
主要实现语言：Rust<br>
以ECS [(Entity Component System)](https://mp.weixin.qq.com/s/dfEyst39sZ1fRCV6hcqCDA)架构，实现游戏世界中的实体、组件、系统。<br>
游戏世界由方块组成，玩家可以自由地放置、破坏方块。<br>
游戏内有天然建筑，例如自然生成的村庄、城堡、矿道等，利用波函数坍缩噪声技术，生成可拆分与组合的结构性建筑。

## 技术栈
1. Bevy  [游戏引擎](https://bevyengine.org/) <br>
2. Rust [编程语言](https://www.rust-lang.org/zh-CN/) <br>
3. Quic [通信协议](https://tquic.net/zh/docs/intro/) <br>
 https://github.com/genmeta/gm-quic <br>
 https://tquic.net/zh/docs/intro/ <br>
4. 噪声 用于生成地形，例如：<br>
OpenSimplex2 与 FBM 噪声：程序化生成地形 <br>
波函数坍缩噪声：生成可拆分与组合的结构性、模块化建筑<br>
参考地址：<br>
https://lib.rs/crates/noise<br>
https://iquilezles.org/articles/morenoise/<br>
八叉树算法：用于管理大量体素<br>
5. IPV6协议：用于服务器网络通信<br>

## 核心功能 / 计划实现功能
- [ ] 方块放置/破坏<br>
- [ ] 各种优化剔除：  <br>
	遮挡剔除、视锥剔除、LOD / HLOD 技术、八叉树等<br>
- [ ] 方块材质加载<br>
- [ ] 区块系统实现<br>
- [ ] QUIC协议的应用<br>
- [ ] 服务端通信/多人在线<br>
- [ ] AABB 碰撞箱: 实现玩家与物体之间物理碰撞<br>
- [ ] 玩家模型<br>
	待选模型文件格式：GLTF、JSON以及其他格式<br>

![alt text](image.png)