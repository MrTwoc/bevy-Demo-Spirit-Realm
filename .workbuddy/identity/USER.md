# twoc

## 基本信息

- **称呼**: twoc
- **角色**: 项目 Owner / Rust 开发者
- **沟通语言**: 中文

## 工作偏好

### 代码交付
- 要求**完整可运行代码**，不接受仅解释性片段
- 偏好先分析→确认→再执行的流程
- 优化建议需附带 CPU/GPU 可行性分析

### 分析风格
- 结构化优先：表格、分层、清单
- 性能优化需要量化支撑（不是"可能更快"）
- 行级定位：说清改哪个文件的哪段代码

### 验证方式
- 自行执行 `cargo check` / `cargo run` 验证
- 我不会代跑编译
- 运行时错误/panic 由 twoc 报告，我做针对性修复

### 技术栈
- Rust + Bevy 0.18 ECS
- Avian3D 物理引擎 + bevy-tnua 角色控制
- WGSL 着色器
- 体素引擎全栈（chunk / mesh / LOD / collision / SVO）

## 项目约定

> 详见 `.workbuddy/memory/MEMORY.md`，以下是关键要点：

- chunk_manager 拆分为 5 个细粒度系统，First/Update 分组调度
- 使用 Bevy Mesh 路径（路径 A），Indirect Draw（路径 B）已移除
- 顶点归一化 + GPU Transform.scale 做 LOD 缩放
- 列扫描优化 mesh 生成，减少 50-70% chunk.get() 调用
- Mesh 创建仅在 Handle 变化时 insert，减少 Command 开销
- 加载队列用 OnceLock 预计算偏移量表
- 不执行 cargo check，由用户手动验证
