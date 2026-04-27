//! 应用扩展存储（AnyMap）。
//!
//! 提供基于 `std::any::TypeId` 的类型安全擦除存储，允许在 `Application` 上
//! 挂载任意插件自定义数据，实现插件间的上下文共享与解耦。

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// 类型擦除扩展存储容器。
///
/// 通过 `TypeId` 作为键，支持存储和检索任意满足 `'static + Send` 的类型。
/// 这是实现插件上下文传递的基础设施。
pub struct Extensions {
    map: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl Extensions {
    /// 创建空的扩展存储。
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// 插入一个扩展值。
    ///
    /// 如果同类型已存在，返回旧值；否则返回 `None`。
    pub fn insert<T: Any + Send>(&mut self, val: T) -> Option<T> {
        let old = self.map.remove(&TypeId::of::<T>());
        self.map.insert(TypeId::of::<T>(), Box::new(val));
        old.and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    /// 获取不可变的扩展引用。
    pub fn get<T: Any + Send>(&self) -> Option<&T> {
        self.map.get(&TypeId::of::<T>())?.downcast_ref::<T>()
    }

    /// 获取可变的扩展引用。
    pub fn get_mut<T: Any + Send>(&mut self) -> Option<&mut T> {
        self.map.get_mut(&TypeId::of::<T>())?.downcast_mut::<T>()
    }

    /// 移除并返回指定类型的扩展值。
    pub fn remove<T: Any + Send>(&mut self) -> Option<T> {
        let boxed = self.map.remove(&TypeId::of::<T>())?;
        boxed.downcast::<T>().ok().map(|b| *b)
    }

    /// 检查是否包含指定类型的扩展。
    pub fn contains<T: Any + Send>(&self) -> bool {
        self.map.contains_key(&TypeId::of::<T>())
    }
}

impl Default for Extensions {
    fn default() -> Self {
        Self::new()
    }
}
