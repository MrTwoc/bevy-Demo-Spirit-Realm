//! Section 生命周期管理
//!
//! 参考 voxy-dev ActiveSectionTracker:
//! - 带 LRU 缓存的 section 缓存池
//! - 引用计数管理
//! - 二级缓存 (secondary cache) 减少重加载

use bevy::prelude::*;
use std::collections::HashMap;
use crate::svo::section::{Section, SectionCoord, SectionId};
use crate::svo::config::SECONDARY_CACHE_SIZE;

/// Section 缓存条目
struct CacheEntry {
    section: Section,
    /// 在 LRU 链表中的前后指针
    prev: Option<SectionId>,
    next: Option<SectionId>,
}

/// Section 生命周期追踪器
#[derive(Resource)]
pub struct SectionTracker {
    /// 活跃 section 表 (ref_count > 0)
    active: HashMap<SectionId, Section>,
    /// LRU 缓存 (ref_count == 0, 暂时保留)
    cache: HashMap<SectionId, Section>,
    /// LRU 链表头 (最近使用)
    lru_head: Option<SectionId>,
    /// LRU 链表尾 (最久未使用)
    lru_tail: Option<SectionId>,
    /// LRU 链表 (用于驱逐)
    lru_prev: HashMap<SectionId, Option<SectionId>>,
    lru_next: HashMap<SectionId, Option<SectionId>>,

    // 回调
    on_unload: Option<Box<dyn FnMut(SectionId) + Send + Sync>>,
}

impl Default for SectionTracker {
    fn default() -> Self {
        Self {
            active: HashMap::new(),
            cache: HashMap::new(),
            lru_head: None,
            lru_tail: None,
            lru_prev: HashMap::new(),
            lru_next: HashMap::new(),
            on_unload: None,
        }
    }
}

impl SectionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置卸载回调
    pub fn set_on_unload<F>(&mut self, callback: F)
    where
        F: FnMut(SectionId) + Send + Sync + 'static,
    {
        self.on_unload = Some(Box::new(callback));
    }

    /// 获取或创建 section (引用计数 +1)
    pub fn acquire(&mut self, coord: SectionCoord) -> &mut Section {
        let id = coord.encode();

        // 检查活跃表
        if self.active.contains_key(&id) {
            let section = self.active.get_mut(&id).unwrap();
            section.ref_count += 1;
            return section;
        }

        // 从 LRU 缓存转移
        let section = if let Some(mut cached) = self.cache.remove(&id) {
            self.remove_from_lru(id);
            cached.ref_count = 1;
            cached
        } else {
            Section::new(coord)
        };

        self.active.insert(id, section);
        self.active.get_mut(&id).unwrap()
    }

    /// 释放引用
    pub fn release(&mut self, id: SectionId) {
        let section = match self.active.get_mut(&id) {
            Some(s) => s,
            None => return,
        };

        section.ref_count -= 1;
        if section.ref_count > 0 {
            return;
        }

        // ref_count 降到 0，移到 LRU 缓存
        let mut section = self.active.remove(&id).unwrap();

        // 如果缓存满了，驱逐最久未用的
        if self.cache.len() >= SECONDARY_CACHE_SIZE {
            if let Some(tail) = self.lru_tail {
                self.evict(tail);
            }
        }

        // 插入到 LRU 头部
        section.is_dirty = false; // 进入缓存时清除脏标志
        self.cache.insert(id, section);
        self.push_lru_front(id);
    }

    /// 获取活跃 section (不增加引用计数)
    pub fn get_active(&self, id: SectionId) -> Option<&Section> {
        self.active.get(&id)
    }

    /// 获取活跃 section 可变引用
    pub fn get_active_mut(&mut self, id: SectionId) -> Option<&mut Section> {
        self.active.get_mut(&id)
    }

    /// 检查 section 是否在缓存中
    pub fn is_cached(&self, id: SectionId) -> bool {
        self.cache.contains_key(&id) || self.active.contains_key(&id)
    }

    /// 从 LRU 缓存中获取 (用于网格生成)
    pub fn get_cached(&self, id: SectionId) -> Option<&Section> {
        self.cache.get(&id)
    }

    /// 标记 section 为脏 (触发重新生成)
    pub fn mark_dirty(&mut self, id: SectionId) {
        if let Some(section) = self.active.get_mut(&id) {
            section.is_dirty = true;
        }
    }

    /// 释放所有资源
    pub fn clear(&mut self) {
        // 触发卸载回调
        if let Some(ref mut cb) = self.on_unload {
            for id in self.active.keys() {
                cb(*id);
            }
            for id in self.cache.keys() {
                cb(*id);
            }
        }
        self.active.clear();
        self.cache.clear();
        self.lru_head = None;
        self.lru_tail = None;
        self.lru_prev.clear();
        self.lru_next.clear();
    }

    /// 当前活跃 section 数
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// 缓存中 section 数
    pub fn cached_count(&self) -> usize {
        self.cache.len()
    }

    // --- LRU 链表操作 ---

    fn push_lru_front(&mut self, id: SectionId) {
        let prev_head = self.lru_head;
        self.lru_head = Some(id);
        self.lru_prev.insert(id, None);
        self.lru_next.insert(id, prev_head);

        if let Some(old_head) = prev_head {
            self.lru_prev.insert(old_head, Some(id));
        } else {
            self.lru_tail = Some(id);
        }
    }

    fn remove_from_lru(&mut self, id: SectionId) {
        let prev = self.lru_prev.remove(&id).flatten();
        let next = self.lru_next.remove(&id).flatten();

        if let Some(p) = prev {
            self.lru_next.insert(p, next);
        } else {
            self.lru_head = next;
        }

        if let Some(n) = next {
            self.lru_prev.insert(n, prev);
        } else {
            self.lru_tail = prev;
        }
    }

    fn evict(&mut self, id: SectionId) {
        if let Some(mut section) = self.cache.remove(&id) {
            self.remove_from_lru(id);
            if let Some(ref mut cb) = self.on_unload {
                cb(id);
            }
            // Section drop 时会释放 voxel Vec
        }
    }
}
