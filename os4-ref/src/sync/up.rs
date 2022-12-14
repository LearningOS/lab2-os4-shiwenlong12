//! Uniprocessor interior mutability primitives
//单处理器内部可变原语 

use core::cell::{RefCell, RefMut};

//将静态数据结构包装在其中，这样我们就可以在没有任何“不安全”的情况下访问它。
//我们应该只在单处理器中使用它。为了获取内部数据的可变引用，请调用`exclusive_access`。
pub struct UPSafeCell<T> {
    /// inner data
    inner: RefCell<T>,
}

unsafe impl<T> Sync for UPSafeCell<T> {}

impl<T> UPSafeCell<T> {
    //用户负责保证内部结构仅在单处理器中使用。
    pub unsafe fn new(value: T) -> Self {
        Self {
            inner: RefCell::new(value),
        }
    }
    /// Panic if the data has been borrowed.
    //如果数据已被借用则Panic
    pub fn exclusive_access(&self) -> RefMut<'_, T> {
        self.inner.borrow_mut()
    }
}
