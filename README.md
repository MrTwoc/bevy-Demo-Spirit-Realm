## 介绍
项目创建时间：<br>
2024年10月左右<br>
$~~~~$本项目是个人学习Rust和Bevy引擎的实践项目，将会持续学习&更新，以完成所有计划内功能为目标。<br>
使用Bevy引擎+Rust编程语言，实现的一个简单的3D体素沙盒类游戏。<br>
主要实现语言：<br>
Rust<br>
$~~~~$以ECS [(Entity Component System)](https://mp.weixin.qq.com/s/dfEyst39sZ1fRCV6hcqCDA)架构，实现游戏世界中的实体、组件、系统。<br>
游戏世界由方块组成，玩家可以自由地放置、破坏方块。<br>
游戏内有天然建筑，例如自然生成的村庄、城堡、矿道等，利用波函数坍缩噪声技术，生成可拆分与组合的结构性建筑。

## 项目记录
同时也是我的个人博客：<br>
https://mrtowc.xlog.app/Spirit-Realm <br>

## 技术栈
1. Bevy  [游戏引擎](https://bevyengine.org/) <br>
2. Rust [编程语言](https://www.rust-lang.org/zh-CN/) <br>
2.1. [tokio](https://tokio.rs/tokio/tutorial/spawning)<br>
2.2. [rayon](https://crates.io/crates/rayon) <br>
3. Quic [通信协议](https://tquic.net/zh/docs/intro/) <br>
 https://github.com/genmeta/gm-quic <br>
 https://tquic.net/zh/docs/intro/ <br>
4. 噪声 用于生成地形，例如：<br>
OpenSimplex2 与 FBM 噪声：程序化生成地表地形 <br>
波函数坍缩噪声：生成可拆分与组合的结构化、模块化建筑<br>
参考地址：<br>
https://lib.rs/crates/noise<br>
https://iquilezles.org/articles/morenoise/<br>
八叉树算法：用于管理大量体素<br>
5. IPV6协议：用于服务器网络通信<br>
6. Protobuf / FlatBuffers [数据序列化协议](https://developers.google.com/protocol-buffers/) <br>

## 核心功能 / 计划实现功能
- [ ] 超平坦世界生成<br>
- [ ] 噪声世界生成<br>
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
## 阶段展示：<br>
一、绘制基础mesh面，无剔除
![alt text](image.png)
二、将六个面都被遮挡的方块整体剔除<br>
但仍旧保留了与空气接触的方块的不可见的mesh面<br>
![alt text](image-1.png)
三、利用OpenSimplex2噪声多次叠加形成分型布朗运噪声(FBm)生成512x32x512大小的地形
![!\[alt text\](91c29d57bba658bb8c90ede6be4be8a.png)](simplex-FBm.png)