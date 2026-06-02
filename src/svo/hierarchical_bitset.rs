//! 分层位集合 (Hierarchical Bit Set)
//!
//! 移植自 Voxy 的 HierarchicalBitSet 实现：
//! - 4 层位数组结构，支持快速查找下一个空闲位
//! - 支持单个分配和连续分配
//! - 时间复杂度：O(1) 分配，O(1) 释放
//!
//! 容量：64^4 = 16,777,216 个位

/// 分层位集合
///
/// 使用 4 层位数组实现高效的位分配：
/// - A: 1 个 u64 (64 位)
/// - B: 64 个 u64 (64² 位)
/// - C: 64² 个 u64 (64³ 位)
/// - D: 64³ 个 u64 (64⁴ 位)
pub struct HierarchicalBitSet {
    /// 容量限制
    limit: u32,
    /// 当前已分配数量
    count: u32,
    /// 第 4 层 (最底层，每个位代表一个可分配位置)
    d: Vec<u64>,
    /// 第 3 层
    c: Vec<u64>,
    /// 第 2 层
    b: Vec<u64>,
    /// 第 1 层 (顶层)
    a: u64,
    /// 最后一个已分配的 ID (用于快速顺序分配)
    end_id: i32,
}

/// 分配失败时返回的常量
pub const SET_FULL: u32 = u32::MAX;

impl HierarchicalBitSet {
    /// 创建新的分层位集合
    ///
    /// # Arguments
    /// * `limit` - 最大可分配数量，不能超过 64^4 = 16,777,216
    pub fn new(limit: u32) -> Self {
        assert!(limit <= (1 << 24), "Limit greater than capacity (64^4)");

        Self {
            limit,
            count: 0,
            d: vec![0u64; 64 * 64 * 64],
            c: vec![0u64; 64 * 64],
            b: vec![0u64; 64],
            a: 0,
            end_id: -1,
        }
    }

    /// 创建默认容量的分层位集合 (64^4 = 16,777,216)
    pub fn default_capacity() -> Self {
        Self::new(1 << 24)
    }

    /// 分配下一个空闲位，返回分配的索引
    ///
    /// 如果没有空闲位或达到限制，返回 SET_FULL
    pub fn allocate_next(&mut self) -> u32 {
        // 检查顶层是否已满
        if self.a == u64::MAX {
            return SET_FULL;
        }

        // 检查是否达到限制
        if self.count + 1 > self.limit {
            return SET_FULL;
        }

        // 在 A 层找到第一个未设置的位
        let a_idx = self.a.trailing_ones() as usize;

        // 在 B 层找到第一个未设置的位
        let b_val = self.b[a_idx];
        let b_idx = b_val.trailing_ones() as usize;
        let mut idx = a_idx * 64 + b_idx;

        // 在 C 层找到第一个未设置的位
        let c_val = self.c[idx];
        let c_idx = c_val.trailing_ones() as usize;
        idx = idx * 64 + c_idx;

        // 在 D 层找到第一个未设置的位
        let d_val = self.d[idx];
        let d_idx = d_val.trailing_ones() as usize;
        idx = idx * 64 + d_idx;

        let ret = idx as u32;

        // 设置 D 层的位
        let d_pos = idx >> 6;
        let d_bit = idx & 0x3F;
        self.d[d_pos] |= 1u64 << d_bit;

        // 如果 D 层的 u64 已满，更新上层
        if self.d[d_pos] == u64::MAX {
            let c_pos = d_pos >> 6;
            let c_bit = d_pos & 0x3F;
            self.c[c_pos] |= 1u64 << c_bit;

            if self.c[c_pos] == u64::MAX {
                let b_pos = c_pos >> 6;
                let b_bit = c_pos & 0x3F;
                self.b[b_pos] |= 1u64 << b_bit;

                if self.b[b_pos] == u64::MAX {
                    let a_bit = b_pos & 0x3F;
                    self.a |= 1u64 << a_bit;
                }
            }
        }

        self.count += 1;

        // 更新 end_id
        if self.end_id < 0 || ret == (self.end_id as u32) + 1 {
            self.end_id = ret as i32;
        }

        ret
    }

    /// 分配连续的 N 个位，返回起始索引
    ///
    /// # Arguments
    /// * `count` - 需要连续分配的数量，不能超过 64
    ///
    /// 如果没有足够的连续空闲位或达到限制，返回 SET_FULL
    pub fn allocate_next_consecutive(&mut self, count: u32) -> u32 {
        assert!(count <= 64, "Count too large for current implementation");

        // 检查是否达到限制
        if self.a == u64::MAX {
            return SET_FULL;
        }
        if self.count + count > self.limit {
            return SET_FULL;
        }

        let check_mask = (1u64 << count) - 1;
        let mut i = self.find_next_free(0) as usize;

        loop {
            // 检查从位置 i 开始是否有 count 个连续的空闲位
            let d_word = i >> 6;
            let d_bit = i & 0x3F;

            let mut fused_value = self.d[d_word] >> d_bit;

            // 如果跨越了 u64 边界，需要融合下一个 u64
            if 64 - d_bit < count as usize {
                if d_word + 1 < self.d.len() {
                    fused_value |= self.d[d_word + 1] << (64 - d_bit);
                }
            }

            // 检查是否有足够的连续空闲位
            if (fused_value & check_mask) != 0 {
                // 跳到下一个空闲位置
                let skip = fused_value.trailing_zeros();
                i += skip as usize;
                i = self.find_next_free(i as u32) as usize;
                continue;
            }

            // 找到足够的连续空闲位，设置它们
            for j in 0..count as usize {
                self.set(i + j);
            }

            return i as u32;
        }
    }

    /// 释放指定索引的位
    ///
    /// 返回 true 如果该位之前是已设置的
    pub fn free(&mut self, idx: u32) -> bool {
        let idx = idx as usize;
        let d_pos = idx >> 6;
        let d_bit = idx & 0x3F;

        let v = self.d[d_pos];
        let was_set = (v & (1u64 << d_bit)) != 0;

        if was_set {
            self.count -= 1;

            // 更新 end_id
            if idx as i32 == self.end_id {
                // 回溯找到最后一个已设置的位
                self.end_id -= 1;
                while self.end_id >= 0 && !self.is_set(self.end_id as u32) {
                    self.end_id -= 1;
                }
            }
        }

        // 清除 D 层的位
        self.d[d_pos] = v & !(1u64 << d_bit);

        // 清除上层的位
        let c_pos = d_pos >> 6;
        let c_bit = d_pos & 0x3F;
        self.c[c_pos] &= !(1u64 << c_bit);

        let b_pos = c_pos >> 6;
        let b_bit = c_pos & 0x3F;
        self.b[b_pos] &= !(1u64 << b_bit);

        let a_bit = b_pos & 0x3F;
        self.a &= !(1u64 << a_bit);

        was_set
    }

    /// 检查指定位是否已设置
    pub fn is_set(&self, idx: u32) -> bool {
        let idx = idx as usize;
        let d_pos = idx >> 6;
        let d_bit = idx & 0x3F;
        (self.d[d_pos] & (1u64 << d_bit)) != 0
    }

    /// 获取当前已分配数量
    pub fn count(&self) -> u32 {
        self.count
    }

    /// 获取容量限制
    pub fn limit(&self) -> u32 {
        self.limit
    }

    /// 获取最大已分配索引
    pub fn max_index(&self) -> i32 {
        self.end_id
    }

    /// 查找从 idx 开始的下一个空闲位
    fn find_next_free(&self, start: u32) -> u32 {
        let mut idx = start as usize;

        loop {
            // 在 A 层查找
            let a_mask = !self.a & (!0u64 << (idx >> 18));
            if a_mask == 0 {
                // 没有空闲位了
                return SET_FULL;
            }
            let a_idx = a_mask.trailing_zeros() as usize;
            idx = (a_idx << 18).max(idx);

            // 在 B 层查找
            let b_mask = !self.b[a_idx] & (!0u64 << ((idx >> 12) & 0x3F));
            if b_mask == 0 {
                // 继续下一个 A 层位
                idx = (a_idx + 1) << 18;
                continue;
            }
            let b_idx = b_mask.trailing_zeros() as usize;
            idx = ((a_idx * 64 + b_idx) << 12).max(idx);

            // 在 C 层查找
            let c_idx_global = idx >> 12;
            let c_mask = !self.c[c_idx_global] & (!0u64 << ((idx >> 6) & 0x3F));
            if c_mask == 0 {
                // 继续下一个 B 层位
                idx = (c_idx_global + 1) << 12;
                continue;
            }
            let c_idx = c_mask.trailing_zeros() as usize;
            idx = ((c_idx_global * 64 + c_idx) << 6).max(idx);

            // 在 D 层查找
            let d_idx_global = idx >> 6;
            let d_mask = !self.d[d_idx_global] & (!0u64 << (idx & 0x3F));
            if d_mask == 0 {
                // 继续下一个 C 层位
                idx = (d_idx_global + 1) << 6;
                continue;
            }
            let d_idx = d_mask.trailing_zeros() as usize;
            idx = (d_idx_global * 64 + d_idx).max(idx);

            return idx as u32;
        }
    }

    /// 设置指定位（内部使用）
    fn set(&mut self, idx: usize) {
        let d_pos = idx >> 6;
        let d_bit = idx & 0x3F;

        // 设置 D 层的位
        let d_val = self.d[d_pos] | (1u64 << d_bit);
        self.d[d_pos] = d_val;

        // 如果 D 层的 u64 已满，更新上层
        if d_val == u64::MAX {
            let c_pos = d_pos >> 6;
            let c_bit = d_pos & 0x3F;
            let c_val = self.c[c_pos] | (1u64 << c_bit);
            self.c[c_pos] = c_val;

            if c_val == u64::MAX {
                let b_pos = c_pos >> 6;
                let b_bit = c_pos & 0x3F;
                let b_val = self.b[b_pos] | (1u64 << b_bit);
                self.b[b_pos] = b_val;

                if b_val == u64::MAX {
                    let a_bit = b_pos & 0x3F;
                    self.a |= 1u64 << a_bit;
                }
            }
        }

        self.count += 1;

        // 更新 end_id
        if idx as i32 == self.end_id + 1 {
            self.end_id = idx as i32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_allocation() {
        let mut bitset = HierarchicalBitSet::new(1024);

        // 分配第一个位
        let id = bitset.allocate_next();
        assert_eq!(id, 0);
        assert!(bitset.is_set(0));
        assert_eq!(bitset.count(), 1);

        // 分配第二个位
        let id = bitset.allocate_next();
        assert_eq!(id, 1);
        assert!(bitset.is_set(1));
        assert_eq!(bitset.count(), 2);
    }

    #[test]
    fn test_free() {
        let mut bitset = HierarchicalBitSet::new(1024);

        // 分配并释放
        let id = bitset.allocate_next();
        assert_eq!(id, 0);

        let was_set = bitset.free(0);
        assert!(was_set);
        assert!(!bitset.is_set(0));
        assert_eq!(bitset.count(), 0);

        // 释放后可以重新分配
        let id = bitset.allocate_next();
        assert_eq!(id, 0);
    }

    #[test]
    fn test_consecutive_allocation() {
        let mut bitset = HierarchicalBitSet::new(1024);

        // 分配连续的 4 个位
        let base = bitset.allocate_next_consecutive(4);
        assert_eq!(base, 0);
        assert_eq!(bitset.count(), 4);

        // 验证所有位都已设置
        for i in 0..4 {
            assert!(bitset.is_set(i));
        }
    }

    #[test]
    fn test_sequential_allocation() {
        let mut bitset = HierarchicalBitSet::new(1024);

        // 顺序分配 100 个位
        for i in 0..100 {
            let id = bitset.allocate_next();
            assert_eq!(id, i);
        }

        assert_eq!(bitset.count(), 100);
        assert_eq!(bitset.max_index(), 99);
    }

    #[test]
    fn test_free_and_realloc() {
        let mut bitset = HierarchicalBitSet::new(1024);

        // 分配一些位
        for _ in 0..10 {
            bitset.allocate_next();
        }

        // 释放中间的位
        bitset.free(5);
        bitset.free(7);

        // 重新分配，应该返回释放的位
        let id = bitset.allocate_next();
        assert_eq!(id, 5); // 最先释放的位

        let id = bitset.allocate_next();
        assert_eq!(id, 7); // 第二个释放的位
    }

    #[test]
    fn test_limit() {
        let mut bitset = HierarchicalBitSet::new(10);

        // 分配到限制
        for _ in 0..10 {
            let id = bitset.allocate_next();
            assert_ne!(id, SET_FULL);
        }

        // 超过限制应该失败
        let id = bitset.allocate_next();
        assert_eq!(id, SET_FULL);
    }

    #[test]
    fn test_large_allocation() {
        let mut bitset = HierarchicalBitSet::new(1024);

        // 分配 1024 个位
        for i in 0..1024 {
            let id = bitset.allocate_next();
            assert_eq!(id, i);
        }

        assert_eq!(bitset.count(), 1024);
        assert_eq!(bitset.max_index(), 1023);

        // 释放所有位
        for i in 0..1024 {
            bitset.free(i);
        }

        assert_eq!(bitset.count(), 0);
        assert_eq!(bitset.max_index(), -1);
    }
}
