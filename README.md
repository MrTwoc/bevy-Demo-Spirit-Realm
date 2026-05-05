## 介绍
现在是2026年5月2日，项目计划近期开始学习&更新。
更新介绍文章将放在个人博客：https://www.twocblog.site/<br>
### 项目创建时间：<br>
2024年11月<br>
$~~~~$本项目是个人学习Rust和Bevy引擎的实践项目，将会持续学习&更新，以完成所有计划内功能为目标。<br>
使用Bevy引擎+Rust编程语言，实现的一个简单的3D体素沙盒类游戏。<br>
主要实现语言：<br>
Rust<br>
$~~~~$以ECS [(Entity Component System)](https://mp.weixin.qq.com/s/dfEyst39sZ1fRCV6hcqCDA)架构，实现游戏世界中的实体、组件、系统。<br>
游戏世界由方块组成，玩家可以自由地放置、破坏方块。<br>
游戏内有天然建筑，例如自然生成的村庄、城堡、矿道等，利用波函数坍缩噪声技术，生成可拆分与组合的结构性建筑。

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
八叉树算法：用于高效管理大量体素<br>
示例：[我是如何把2的60次方的方块画到屏幕上的【算法解析】](https://www.bilibili.com/video/BV1he411q7Zi/?spm_id_from=333.1387.homepage.video_card.click&vd_source=511b084e4bf87d71d725c5db0fb20b7f)
<br>
5. IPV6协议：用于服务器网络通信<br>
6. Protobuf / FlatBuffers [数据序列化协议](https://developers.google.com/protocol-buffers/) <br>

## 核心功能 / 计划实现功能
- [ ] 超平坦世界生成<br>
- [ ] 噪声世界生成<br>
- [ ] 方块放置/破坏<br>
- [ ] 方块材质加载<br>
- [ ] 区块系统实现<br>

## MVP实现步骤
1. 画一个方块，生成一个摄像机，将摄像机指向方块
2. 画一个区块，尺寸为32x32x32，将方块添加到区块中
3. 实现摄像机移动功能，玩家可以自由地移动摄像机，查看游戏世界
4. 实现方块放置/破坏功能
