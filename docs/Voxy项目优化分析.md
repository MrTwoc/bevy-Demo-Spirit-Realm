# Voxy 项目优化分析

**日期：2026年5月29日**

---

## 一、项目概述

voxy-dev 是一个 Minecraft 体素渲染引擎，核心架构为 **GPU 驱动的层级八叉树遍历系统**。节点按 LOD 层级组织，通过 Compute Shader 实现全 GPU 端遍历与剔除，并结合 CPU 端的异步节点管理和几何体回收，达成大视距下的高性能渲染。

渲染管线核心流程：

```
RingTracker (CPU, 圆形范围加载管理)
  → NodeManager (CPU, 节点创建/扩容/删除)
    → HierarchicalOcclusionTraverser (GPU, 全 GPU 层级遍历)
      → MDICSectionRenderer (GPU, 遮挡剔除 + MultiDrawIndirectCount 命令生成)
        → NodeCleaner (GPU, 几何体回收)
```

---

## 二、区块加载剔除（Distance-based Culling）

### 2.1 RingTracker — 圆形距离加载管理器

**文件位置：** `src/main/java/me/cortex/voxy/client/core/util/RingTracker.java`

`RingTracker` 是**纯 CPU 端**的加载范围管理器，跟踪相机位置在 `>>9`（即除以 512）缩放的区块网格上的移动。

#### 核心机制

1. **构造时**传入半径 `radius`（对应 `sectionRenderDistance`），计算出圆形范围内的所有 `(x, z)` 坐标对。

2. `generateBoundingHalfCircleDistance()` 对每一行 `x` 计算该行在圆内的 `z` 范围：

   ```java
   boundDist[i] = (int) Math.sqrt(radius * radius - i * i)
   ```

3. `moveCenter(x, z)` — 相机移动时，**只处理进出圆环的增量行/列**，不重建整个集合。增量移动通过 `moveX()` / `moveZ()` 实现，只扫描新增和消失的列。

4. `process(N, onAdd, onRemove)` — 每帧按 `processRate` 处理 N 个 add/remove 操作，**平滑控制加载节奏**，防止一帧内大量加载导致卡顿。

#### 性能优势

- 增量更新 O(radius) 而非 O(radius²)
- 平滑加载避免瞬时卡顿
- 仅在 `CHECK_DISTANCE_BLOCKS`（128格）距离变化后才移动中心，减少频繁重算

---

### 2.2 RenderDistanceTracker — 渲染距离适配器

**文件位置：** `src/main/java/me/cortex/voxy/client/core/rendering/RenderDistanceTracker.java`

`RenderDistanceTracker` 将 `RingTracker` 的 `(x, z)` 网格坐标**扩展为全 Y 轴范围**（`minSec` ~ `maxSec`），然后调用 `addTopLevelNode` / `removeTopLevelNode`。

```java
// 每个 (x,z) 位置, 遍历 y 范围, 生成 TopLevel 节点
private void add(int x, int z) {
    for (int y = this.minSec; y <= this.maxSec; y++) {
        this.addTopLevelNode.accept(WorldEngine.getWorldSectionId(4, x, y, z));
    }
}
```

关键细节：`getWorldSectionId(4, x, y, z)` 中的 **层级 4 是顶层**，RingTracker 操作的是 Top-Level 节点，而不是最细粒度区块。这意味着渲染范围管理是在八叉树最粗粒度层面进行的。

---

## 三、节点管理与保留（CPU 端）

### 3.1 NodeManager — 核心节点管理器

**文件位置：** `src/main/java/me/cortex/voxy/client/core/rendering/hierachical/NodeManager.java`

#### 节点类型体系

| 类型 | 标识位 | 含义 |
|------|--------|------|
| `NODE_TYPE_LEAF` | `0b00<<30` | 叶子节点：有几何体（可能为空），无子节点 |
| `NODE_TYPE_INNER` | `0b01<<30` | 内部节点：有子节点，可能有自身上采样几何体 |
| `NODE_TYPE_REQUEST_SINGLE` | `0b10<<30 \| 0b0<<29` | 单请求：TopLevel 节点正在加载初始数据 |
| `NODE_TYPE_REQUEST_CHILD` | `0b10<<30 \| 0b1<<29` | 子请求：内部节点正在扩展子节点 |

#### 节点数据存储（NodeStore）

每个节点存储 4 个 `long`（32字节）：

| 位域 | 内容 |
|------|------|
| `localNodeData[idx+0]` | 世界位置 `position` |
| `localNodeData[idx+1]` | 几何体指针(0-23bit) + 子节点指针(24-47bit) + 子节点存在性掩码(48-55bit) + 子节点指针数(56-58bit) + 请求InFlight标记(59bit) + 节点类型(61-62bit) |
| `localNodeData[idx+2]` | 请求ID(0-18bit) + 全子节点为叶标记(19bit) |
| `localNodeData[idx+3]` | 保留/标志位 |

#### 关键操作流程

**TopLevel 节点插入（`insertTopLevelNode`）：**

1. 验证位置合法性（`assertPosValid`）
2. 检查 `activeSectionMap` 防止重复插入
3. 创建 `SingleNodeRequest`
4. 调用 `watcher.watch()` 注册世界更新监听
5. 记入 `activeSectionMap` 和 `topLevelNodes`

**节点扩容触发（`processRequest`）：**

当 GPU 遍历时发现某个节点需要进一步细分但没有子节点时，通过 `requestQueue` 传回 CPU，由 `AsyncNodeManager` 异步处理：

- **叶子节点** → `makeLeafChildRequest()`: 根据 `childExistence` 掩码创建子节点请求，对每个存在的子节点调用 `watcher.watch()` 注册更新监听。
- **内部节点** → `processInnerRequest()`: 如果没有几何体上采样数据，注册 `UPDATE_TYPE_BLOCK_BIT` 监听几何体更新。

**子节点增删（`processChildChange`）：**

当世界被修改（区块内容变化），`SectionUpdateRouter` 转发 `childExistence` 变更：

- `add != 0`: 向已有请求添加新子节点或新建请求
- `rem != 0`: 从请求中移除子节点，从 `activeSectionMap` 中递归删除子节点子树（`recurseRemoveNode`），释放几何体资源
- `childExistence == 0`: 内部节点退化为叶子节点，移除子节点指针

**请求完成（`finishRequest`）：**

- **叶子扩容** → 分配子节点 ID，设置位置/几何体/子节点存在性掩码，父节点从 `LEAF` 升格为 `INNER` 类型
- **内部添加** → 交错合并新旧子节点指针，更新 `activeSectionMap` 中的映射

#### 递归删除机制（`_recurseRemoveNode`）

支持两种模式：
- `recurseRemoveNode`: 完全删除节点及所有子节点
- `recurseRemoveChildNodes`: 只删除子节点，保留当前节点

删除流程：
1. 取消 `watcher` 中的更新监听
2. 释放几何体到 `GeometryCache`（`removeGeometryCached`）
3. 释放 NodeStore 中的节点槽位
4. 更新 GPU 端的 `nodeUpdates` 集合

---

### 3.2 AsyncNodeManager — 异步节点管理器

**文件位置：** `src/main/java/me/cortex/voxy/client/core/rendering/hierachical/AsyncNodeManager.java`

`AsyncNodeManager` 使用**独立线程**处理所有 Node 操作，通过 `SyncResults` 对象与渲染线程无锁交换。

#### 架构设计

```
渲染线程                    异步线程 (Async Node Manager)
    │                              │
    ├─ submitRequestBatch() ──→    ├─ manager.processRequest()
    ├─ submitChildChange() ──→     ├─ manager.processChildChange()
    ├─ submitGeometryResult() ──→  ├─ manager.processGeometryResult()
    ├─ addTopLevel() ──→           ├─ manager.insertTopLevelNode()
    └─ removeTopLevel() ──→        └─ manager.removeTopLevelNode()
    
    tick() ←── SyncResults (原子交换)
    ├─ 更新几何体 (multiMemcpy Compute Shader)
    ├─ 更新节点数据 (scatterWrite Compute Shader)
    └─ 更新 NodeCleaner visibility IDs
```

#### 关键设计

- **工作计数器** (`workCounter`): 使用 `AtomicInteger`，增量唤醒异步线程
- **LockSupport.park/unpark**: 无工作时线程挂起，有工作时立即唤醒
- **SyncResults**: 封装所有需要同步到渲染线程的数据（几何体上传、节点更新、TLN 变更、清理器操作），通过 `VarHandle` 实现无锁原子交换
- **背压控制**: 当几何体缓冲区空闲 < 50MB 时暂停几何体上传
- **上传节流**: 每次循环最多上传 300 个几何体/1MB 数据，平滑负载

---

### 3.3 GeometryCache — CPU 端 LRU 几何体缓存

**文件位置：** `src/main/java/me/cortex/voxy/client/core/rendering/GeometryCache.java`

```java
public class GeometryCache {
    private final Long2ObjectLinkedOpenHashMap<BuiltSection> cache;
    private long maxCombinedSize;  // 上限 4GB
    private long currentSize;
}
```

- `put()`: 新几何体加入缓存，当 `currentSize > maxCombinedSize` 时逐出最久未访问的条目
- `remove()`: 移除指定位置的缓存（用于世界更新时使缓存失效）
- `worldEvent()` 中先 `geometryCache.clear(section.key)` 再触发重新生成
- 线程安全：使用 `ReentrantLock`

---

### 3.4 SectionUpdateRouter — 世界更新路由器

**文件位置：** `src/main/java/me/cortex/voxy/client/core/rendering/SectionUpdateRouter.java`

路由世界更新事件到正确的子系统：

- `watch()` / `unwatch()`: 管理位置到更新类型的映射，使用 16 个分片锁减少竞争
- `forwardEvent()`: 根据更新类型分发事件
  - `UPDATE_TYPE_BLOCK_BIT` → 触发几何体重新生成
  - `UPDATE_TYPE_CHILD_EXISTENCE_BIT` → 触发子节点变化回调
- `triggerRemesh()`: 邻居区块变化时触发相邻区块重建

---

## 四、几何体回收系统（NodeCleaner）

### 4.1 NodeCleaner — GPU 端逐帧回收

**文件位置：** `src/main/java/me/cortex/voxy/client/core/rendering/hierachical/NodeCleaner.java`

`NodeCleaner` 是**纯 GPU** 的几何体生命周期管理器，用于在几何体缓冲区满时回收最久未渲染的几何体。

#### 触发条件

```java
private boolean shouldCleanGeometry() {
    long remaining = this.nodeManager.getGeometryCapacity()
                   - this.nodeManager.getUsedGeometryCapacity();
    return remaining < 256_000_000; // 空闲空间 < 256MB
}
```

#### 回收流程（3 个 Compute Shader 组成）

**阶段 1：排序（`sort_visibility.comp`）**
- 对所有节点按 `lastRenderFrame` 值排序
- 使用工作组内排序网络 + 快速排序
- 输出最久未渲染的前 `OUTPUT_COUNT`（256）个节点 ID

**阶段 2：结果转换（`result_transformer.comp`）**
- 将排序结果转换为节点位置 `pos = (x<<32)|z`
- 排除当前帧刚被标记可见的节点（`lastRenderFrame == visibilityId`）
- 保留有效节点位置

**阶段 3：CPU 回传与执行**
- 通过 `DownloadStream` 将结果异步下载到 CPU
- 调用 `submitRemoveBatch()` 提交到 `AsyncNodeManager`
- `AsyncNodeManager` 调用 `manager.removeNodeGeometry()` 逐个处理

#### visibility 标记机制

- `visibilityBuffer`: GPU 端每节点一 `uint`，存 `lastRenderFrame`
- 遍历着色器中 `lastRenderFrame[getId(node)] = frameId` 标记可见
- `visibilityId`: 每帧递增的全局标识
- `updateIds()`: 使用 `batch_visibility_set.comp` 批量重置指定节点的可见性标记

---

### 4.2 几何体回收策略（`removeNodeGeometry`）

#### 内部节点回收（`clearGeometryInternal`）

```
1. 获取当前几何体 ID
2. 如果几何体非空/非 EMPTY:
   a. 取消 UPDATE_TYPE_BLOCK_BIT 监听
   b. 移除几何体到缓存 (removeGeometryCached)
   c. 设置几何体为 NULL_GEOMETRY_ID
   d. 标记节点需更新到 GPU
   e. 取消几何体 InFlight 标记
```

#### 叶子节点回收（`processLeafGeometryRemoval`）

```
1. 找到父节点 (makeParentPos)
2. 验证父节点必须是 INNER 类型
3. 获取父节点几何体:
   a. 如果父节点有上采样几何体 (非 NULL):
      - 递归删除所有子节点 (recurseRemoveChildNodes)
      - 将父节点从 INNER 转换为 LEAF
      - 相当于"上采样替代"了细粒度几何体
   b. 如果父节点无几何体 (NULL):
      - 无法直接替代，先请求父节点生成上采样几何体 (processRequest)
```

**顶层叶子节点保护**：TopLevel 节点的几何体**不允许被回收**，因为它们是渲染范围边界的完整表示。

---

## 五、GPU 端遍历与剔除（HierarchicalOcclusionTraverser）

### 5.1 遍历着色器

**文件位置：** `src/main/resources/assets/voxy/shaders/lod/hierarchical/traversal_dev.comp`

全 GPU 实现的 BFS 层级遍历，每帧迭代 `MAX_ITERATIONS` 轮（等于最大 LOD 层数 + 1），每轮处理上一轮输出的节点队列。

#### 遍历主循环

```glsl
void main() {
    uint nodeId = getCurrentNode();
    if (nodeId != SENTINAL_OUT_OF_BOUNDS) {
        UnpackedNode node;
        unpackNode(node, nodeId);
        if (isWithinRenderDistance(node)) {
            traverse(node);
        }
    }
}
```

#### 每个节点的处理流程（`traverse` 函数）

```glsl
void traverse(in UnpackedNode node) {
    setupScreenspace(node);

    if (outsideFrustum() || isCulledByHiz()) {
        // 被剔除,不处理
    } else {
        // 可见
        if (node.lodLevel != 0 && shouldDecend()) {
            // 屏幕空间足够大,需要下行
            if (hasChildren(node)) {
                enqueueChildren(node);  // 有子节点则下行
            } else {
                addRequest(node);       // 无子节点,请求 CPU 扩容
                enqueueSelfForRender(node);  // 同时渲染自身
            }
        } else {
            // 不下行,直接渲染
            if (hasMesh(node)) {
                enqueueSelfForRender(node);
            } else {
                addRequest(node);       // 没有几何体,请求生成
                if (node.lodLevel != 0) {
                    enqueueChildren(node);  // 同时下行查找
                }
            }
        }
    }
}
```

---

### 5.2 四级剔除条件详解

#### 5.2.1 渲染距离剔除

```glsl
bool isWithinRenderDistance(in UnpackedNode node) {
    if (renderDistance < 0.0f) return true;
    vec3 close = closestPointToCamera(node);
    float xzDist = close.x*close.x + close.z*close.z;
    return xzDist <= renderDistance;
}
```

- 计算节点最近点到相机的 XZ 平面距离
- `closestPointToCamera()`: 计算 AABB 到相机的最近点（clamp 到 AABB 范围内）
- 渲染距离为负时禁用此检查

#### 5.2.2 视锥体剔除

```glsl
// outsideFrustum() — 6 视锥面测试
```

- 从 Uniform 中传入 6 个视锥面
- 对节点 AABB 做完全包含测试
- 任一面外侧即剔除

#### 5.2.3 Hi-Z 遮挡剔除

**文件位置：** `src/main/java/me/cortex/voxy/client/core/rendering/util/HiZBuffer.java`

```glsl
// isCulledByHiz() — 从 Hi-Z 缓冲各级 mip 采样
```

Hi-Z 构建流程：

1. 深度缓冲作为 mip 0 输入
2. 使用 `blit.vsh/frag` 着色器逐级生成 mip chain
3. 每级使用 `GL_TEXTURE_BARRIER` 确保一致性
4. 使用 `GL_NEAREST_MIPMAP_NEAREST` 采样策略

遍历着色器中：

- 将节点 AABB 投影到屏幕空间
- 从 Hi-Z 缓冲的适当 mip 级别采样
- 如果节点最近深度 > Hi-Z 值，说明被遮挡

#### 5.2.4 屏幕空间大小（LOD 控制）

```glsl
// shouldDecend() — 对比 minSSS 阈值决定是否下行
```

- 计算节点在屏幕上的投影面积
- 与 `minSSS`（`VoxyConfig.subDivisionSize²`）比较
- 面积足够大时下行细分，否则使用当前 LOD

### 5.3 请求队列自适应调节

```java
// uploadUniform() 中
final double TARGET_COUNT = 4000;
double iFillness = Math.max(0, (TARGET_COUNT - meshGen.getTaskCount()) / TARGET_COUNT);
iFillness = Math.pow(iFillness, 2);
final int requestSize = (int) Math.ceil(iFillness * MAX_REQUEST_QUEUE_SIZE);
```

- 积压多时缩小请求量，防止 CPU 被淹没
- 使用二次函数使调节更敏感
- `requestQueueSize` 传入 GPU 作为软限制

### 5.4 请求回传机制

GPU 端将需要扩容的节点写入 `requestQueue`，然后通过 `DownloadStream` 异步下载到 CPU：

```java
// HierarchicalOcclusionTraverser.downloadResetRequestQueue()
private void forwardDownloadResult(long ptr, long size) {
    int count = MemoryUtil.memGetInt(ptr);
    // ... 边界检查 ...
    var buffer = new MemoryBuffer(count*8L+8).cpyFrom(ptr-8);
    MemoryUtil.memPutInt(buffer.address, count);
    this.nodeManager.submitRequestBatch(buffer);
}
```

---

### 5.5 遍历队列实现

```glsl
// 双缓冲翻转队列
for (int iter = 1; iter < MAX_ITERATIONS; iter++) {
    // 翻转源/目标缓冲
    glBindBufferBase(GL_SHADER_STORAGE_BUFFER, NODE_QUEUE_SOURCE_BINDING,
        ((iter & 1) == 0 ? scratchQueueA : scratchQueueB).id);
    glBindBufferBase(GL_SHADER_STORAGE_BUFFER, NODE_QUEUE_SINK_BINDING,
        ((iter & 1) == 0 ? scratchQueueB : scratchQueueA).id);
    // 间接分派
    glDispatchComputeIndirect(iter * 4 * 4);
}
```

- 每轮迭代使用上一轮的输出作为输入
- 双缓冲翻转避免读写冲突
- 间接分派由 GPU 自行决定工作组数

---

## 六、MDIC 渲染管线的二级剔除

### 6.1 MDICSectionRenderer

**文件位置：** `src/main/java/me/cortex/voxy/client/core/rendering/section/backend/mdic/MDICSectionRenderer.java`

使用 `glMultiDrawElementsIndirectCountARB` 实现高效的批量渲染。

#### buildDrawCalls 流程

在层级遍历完成后做**更精细的遮挡测试**：

**步骤 1：prep.comp — 初始化**
- 初始化 drawCount buffer

**步骤 2：遮挡测试（cull.raster.vert/.frag）**
- 用完整网格做一次深度遮挡测试
- `glColorMask(false)` + `glDepthMask(false)` — 不产生颜色输出
- 使用 `GL_REPRESENTATIVE_FRAGMENT_TEST_NV`（NVIDIA 优化）
- 标记可见性到 `visibilityBuffer`

**步骤 3：cmdgen.comp — 生成绘制命令**
- 根据 `visibilityBuffer` 判断哪些区块可见
- 生成 MDIC 间接绘制命令
- 按区块方向（6面+双面+半透明）生成子命令

**步骤 4：半透明排序**
- 使用 `prefixsum.comp` 计算距离前缀和
- 使用 `buildtranslucents.comp` 按距离排序半透明区块

#### 渲染容量限制

```java
public static final int OPAQUE_DRAW_COUNT    = 400_000;
public static final int TRANSLUCENT_DRAW_COUNT = 100_000;
public static final int TEMPORAL_DRAW_COUNT    = 100_000;
```

---

### 6.2 cmdgen.comp — 绘制命令生成详解

**文件位置：** `src/main/resources/assets/voxy/shaders/lod/gl46/cmdgen.comp`

```glsl
void main() {
    uint sectionId = indirectLookup[gl_GlobalInvocationID.x];
    SectionMeta meta = sectionData[sectionId];
    uint dat = visibilityData[sectionId];

    // 可见性判断: 上次被渲染的帧 ID 与当前帧一致
    bool shouldRender = (dat & 0x7fffffffu) == frameId;
    // 时间性判断: 上上帧未被渲染（新可见）
    bool renderTemporally = (dat & 0x80000000u) == 0;

    if (shouldRender) {
        // 按6个方向生成绘制命令
        // 使用遮挡掩码跳过不可见面
        uint msk = 0;
        msk |= uint(count != 0 && relative.y > -1) << 0;  // 下
        msk |= uint(count != 0 && relative.y <  1) << 1;  // 上
        // ... 南北东西

        uint cmdCnt = bitCount(msk);
        uint cmdPtr = atomicAdd(opaqueDrawCount, cmdCnt);
        // 写入绘制命令
        writeCmd(cmdPtr++, drawId, offset, quadCount);
    }
}
```

关键优化：
- 使用 `relative` 位置判断相邻区块关系，跳过被相邻区块完全遮挡的面
- 半透明区块按距离分桶，使用前缀和实现基数排序

---

## 七、核心数据结构汇总

| 数据结构 | 位置 | 用途 | 线程安全 |
|----------|------|------|----------|
| `RingTracker` | CPU | 圆形加载范围跟踪，增量更新 | 否（渲染线程） |
| `RenderDistanceTracker` | CPU | 将网格坐标扩展为全 Y 轴 TopLevel 节点 | 否（渲染线程） |
| `NodeStore` | CPU | 节点存储，每个节点 4 个 `long`（32字节） | 否（异步线程） |
| `activeSectionMap` | CPU | `pos → nodeId` 映射（`Long2IntOpenHashMap`） | 否（异步线程） |
| `SectionUpdateRouter` | CPU | 世界更新事件路由，16 分片锁 | 是（跨线程） |
| `GeometryCache` | CPU | LRU 几何体缓存（4GB上限） | 是（ReentrantLock） |
| `AsyncNodeManager` | CPU | 异步节点管理，无锁结果同步 | 是（VarHandle） |
| `visibilityBuffer` | GPU | 每节点一 `uint`，存 `lastRenderFrame` | GPU |
| `nodeBuffer` | GPU | 节点数据的 GPU 副本（16字节/节点） | GPU |
| `requestQueue` | GPU→CPU | 需要扩容的节点回传 | 异步下载 |
| `renderQueue` | GPU | 可见节点 Mesh ID 输出 | GPU |
| `drawCallBuffer` | GPU | MDIC 间接绘制命令 | GPU |
| `HiZBuffer` | GPU | 层次深度缓冲，用于遮挡剔除 | GPU |

---

## 八、关键配置参数

| 参数 | 默认值 | 含义 |
|------|--------|------|
| `sectionRenderDistance` | 可配置 | 渲染距离（TopLevel 区块半径） |
| `subDivisionSize` | 可配置 | 屏幕空间细分阈值 |
| `CHECK_DISTANCE_BLOCKS` | 128 | 相机移动多少格才更新 RingTracker |
| `MAX_ITERATIONS` | MAX_LOD_LAYER+1 | 遍历着色器最大迭代轮数 |
| `MAX_REQUEST_QUEUE_SIZE` | 50 | 每帧最大节点扩容请求数 |
| `MAX_QUEUE_SIZE` | 200,000 | 遍历队列最大容量 |
| `OPAQUE_DRAW_COUNT` | 400,000 | 不透明最大绘制调用数 |
| `OUTPUT_COUNT` (NodeCleaner) | 256 | 每帧最多回收的节点数 |
| 清理触发阈值 | 256MB | 几何体缓冲区空闲空间不足时触发清理 |
| 上传暂停阈值 | 50MB | 几何体缓冲区空闲空间不足时暂停上传 |

---

## 九、对当前项目的可迁移要点

### 9.1 双重剔除流水线对比

| 剔除层级 | voxy-dev 实现 | 当前项目状态 |
|----------|--------------|------------|
| 渲染距离 | `RingTracker` → CPU 端圆形范围加载 | 已有 |
| 视锥体 | `outsideFrustum()` — 遍历着色器内联 | 缺失 |
| Hi-Z 遮挡 | `isCulledByHiz()` — 全 GPU mip 测试 | 缺失 |
| 屏幕空间 LOD | `shouldDecend()` — 动态决定是否下行细分 | 缺失 |
| 二级遮挡 | `cull.raster` — 完整网格深度测试 | 缺失 |

### 9.2 几何体回收策略

voxy-dev 的 `NodeCleaner` 在内存压缩问题上有直接参考价值：

- **触发门限**：几何体缓冲区空闲空间 < 256MB 时触发清理
- **排序+截断**：Compute Shader 对所有节点按 `lastRenderFrame` 排序，取前 256 个最久未渲染的节点
- **递归缓解**：`processLeafGeometryRemoval()` 将叶子节点几何体清除后，检查父节点是否有上采样几何体，如果有则将父节点转换为叶子（递归清除所有子节点），实现**上采样替代**细粒度几何体

### 9.3 请求队列自适应调节

```java
iFillness = max(0, (TARGET_COUNT - 积压任务数) / TARGET_COUNT)
requestSize = ceil(iFillness² × MAX_REQUEST_QUEUE_SIZE)
```

积压多时缩小请求量，防止 CPU 被淹没 — 这种**背压控制**在 SVO 请求回路中值得引入。

### 9.4 AsyncNodeManager 架构

所有 CPU 端树状结构操作都在异步线程完成，渲染线程只做 GPU 上传和下发 Compute Dispatch：

- `SyncResults` 对象包含所有需要同步的数据
- 通过 `VarHandle` 原子交换实现无锁同步
- 几何体上传使用 Compute Shader `memcpy.comp` 实现批量 GPU 上传
- 节点更新使用 Compute Shader `scatter.comp` 实现批量 GPU 散射写入

### 9.5 可借鉴的 GPU 技术

| 技术 | 实现位置 | 预期收益 |
|------|----------|----------|
| Hi-Z 层次遮挡 | `HiZBuffer.java` + 遍历着色器 | 大幅减少被遮挡区块的绘制 |
| 屏幕空间 LOD | 遍历着色器 `shouldDecend()` | 远距离区块减少细分 |
| GPU BFS 遍历 | `traversal_dev.comp` | 避免 CPU 遍历开销 |
| MDIC 批量渲染 | `MDICSectionRenderer` | 减少 Draw Call 数量 |
| GPU 排序回收 | `NodeCleaner` 3 个 Compute Shader | 无 CPU 排序开销的几何体回收 |
| Compute 批量上传 | `memcpy.comp` / `scatter.comp` | 减少 CPU↔GPU 同步点 |
