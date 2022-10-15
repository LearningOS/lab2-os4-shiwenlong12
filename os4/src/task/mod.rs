
//任务管理实施
//关于任务管理的所有内容，如启动和切换任务都在这里实现。
//名为“TASK_MANAGER`”的[`TaskManager`]的单个全局实例控制操作系统中的所有任务。
//看到[`__switch`]时要小心。围绕此函数的控制流可能不是您所期望的。

mod context;
mod switch;
#[allow(clippy::module_inception)]
mod task;

use crate::config;
use crate::loader::{get_app_data, get_num_app};
use crate::mm;
use crate::sync::UPSafeCell;
use crate::timer;
use crate::trap::TrapContext;
use alloc::vec::Vec;
use lazy_static::*;
pub use switch::__switch;
pub use task::{TaskControlBlock, TaskStatus};

pub use context::TaskContext;

//任务管理器，用于管理所有任务。
//在“TaskManager”上实现的函数处理所有任务状态转换和任务上下文切换。
//为了方便起见，您可以在模块级别中找到围绕它的包装器。
//大多数`TaskManager`都隐藏在“内部”字段后面，以将借用检查推迟到运行时。
//您可以在`TaskManager`上的现有函数中看到如何使用`inner`的示例。

pub struct TaskManager {
    /// 任务总数
    num_app: usize,
    /// 使用内部值获取可变访问
    inner: UPSafeCell<TaskManagerInner>,
}

/// “UPSafeCell”中的任务管理器内部
struct TaskManagerInner {
    /// task list
    tasks: Vec<TaskControlBlock>,
    /// id of current `Running` task
    current_task: usize,
}

//lazy_static是社区提供的非常强大的宏，用于懒初始化静态变量
//lazy_static允许我们在运行期初始化静态变量！
lazy_static! {
    //通过lazy_static创建一个`TaskManager`实例！
    pub static ref TASK_MANAGER: TaskManager = {
        info!("init TASK_MANAGER");
        let num_app = get_num_app();
        info!("num_app = {}", num_app);
        let mut tasks: Vec<TaskControlBlock> = Vec::new();
        for i in 0..num_app {
            tasks.push(TaskControlBlock::new(get_app_data(i), i));
        }
        TaskManager {
            num_app,
            inner: unsafe {
                UPSafeCell::new(TaskManagerInner {
                    tasks,
                    current_task: 0,
                })
            },
        }
    };
}

impl TaskManager {
     //运行任务列表中的第一个任务。
    //通常，任务列表中的第一个任务是空闲任务（稍后我们称之为零进程）。
    //但在ch4中，我们静态加载应用程序，所以第一个任务是真正的应用程序。
    fn run_first_task(&self) -> ! {
        let mut inner = self.inner.exclusive_access();
        let next_task = &mut inner.tasks[0];
        next_task.task_status = TaskStatus::Running;
        // ehe
        next_task.start_time = timer::get_time_us();

        let next_task_cx_ptr = &next_task.task_cx as *const TaskContext;
        drop(inner);
        let mut _unused = TaskContext::zero_init();
        //在此之前，我们应该删除必须手动删除的局部变量
        unsafe {
            __switch(&mut _unused as *mut _, next_task_cx_ptr);
        }
        panic!("unreachable in run_first_task!");
    }

    //将当前“正在运行”任务的状态更改为“就绪”。 
    fn mark_current_suspended(&self) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].task_status = TaskStatus::Ready;
    }

    //将当前“正在运行”任务的状态更改为“已退出”。
    fn mark_current_exited(&self) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].task_status = TaskStatus::Exited;
    }

    //查找要运行的下一个任务并返回任务id。
    //在这种情况下，我们只返回任务列表中的第一个“就绪”任务。
    fn find_next_task(&self) -> Option<usize> {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        (current + 1..current + self.num_app + 1)
            .map(|id| id % self.num_app)
            .find(|id| inner.tasks[*id].task_status == TaskStatus::Ready)
    }

    /// Get the current 'Running' task's token.
    fn get_current_token(&self) -> usize {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].get_user_token()
    }

    #[allow(clippy::mut_from_ref)]
    /// Get the current 'Running' task's trap contexts.
    fn get_current_trap_cx(&self) -> &mut TrapContext {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].get_trap_cx()
    }

    /// Switch current `Running` task to the task we have found,
    /// or there is no `Ready` task and we can exit with all applications completed
    //将当前“正在运行”任务切换到我们找到的任务，
    //或者没有“就绪”任务，我们可以在完成所有应用程序后退出
    fn run_next_task(&self) {
        if let Some(next) = self.find_next_task() {
            let mut inner = self.inner.exclusive_access();
            let current = inner.current_task;
            inner.tasks[next].task_status = TaskStatus::Running;
            inner.current_task = next;
            // ehe
            if inner.tasks[next].start_time == 0 {
                inner.tasks[next].start_time = timer::get_time_us();
            }

            let current_task_cx_ptr = &mut inner.tasks[current].task_cx as *mut TaskContext;
            let next_task_cx_ptr = &inner.tasks[next].task_cx as *const TaskContext;
            drop(inner);
            // before this, we should drop local variables that must be dropped manually
            //在此之前，我们应该删除必须手动删除的局部变量
            unsafe {
                __switch(current_task_cx_ptr, next_task_cx_ptr);
            }
            // go back to user mode
        } else {
            panic!("All applications completed!");
        }
    }

    /// 更新特定应用的系统调用次数
    fn update_syscall_times(&self, id: usize) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].syscall_times[id] += 1;
    }

    /// 得到系统调用次数
    fn get_syscall_times(&self) -> [u32; 500] {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].syscall_times
    }

    /// 得到当前任务的开始时间
    fn get_start_time(&self) -> usize {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        return timer::get_time_us() - inner.tasks[current].start_time;
    }

    /// mmap
    fn mmap(&self, start: usize, len: usize, port: usize) -> isize {
        if (start % config::PAGE_SIZE != 0) || (port & !0x7 != 0) || (port & 0x7 == 0) {
            return -1;
        }

        let start_address = mm::VirtAddr(start);
        let end_address = mm::VirtAddr(start + len);

        let map_permission =
            mm::MapPermission::from_bits((port as u8) << 1).unwrap() | mm::MapPermission::U;

        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;

        for vpn in mm::VPNRange::new(mm::VirtPageNum::from(start_address), end_address.ceil()) {
            if let Some(pte) = inner.tasks[current].memory_set.translate(vpn) {
                if pte.is_valid() {
                    println!("[debug] This area is used!");
                    return -1;
                }
            };

            println!("[debug] {}", usize::from(vpn));
        }

        inner.tasks[current].memory_set.insert_framed_area(
            start_address,
            end_address,
            map_permission,
        );

        for vpn in mm::VPNRange::new(mm::VirtPageNum::from(start_address), end_address.ceil()) {
            if let None = inner.tasks[current].memory_set.translate(vpn) {
                return -1;
            };
        }

        return 0;
    }

    /// munmap
    fn munmap(&self, start: usize, len: usize) -> isize {
        if start % config::PAGE_SIZE != 0 {
            return -1;
        }

        let start_address = mm::VirtAddr(start);
        let end_address = mm::VirtAddr(start + len);

        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;

        for vpn in mm::VPNRange::new(mm::VirtPageNum::from(start_address), end_address.ceil()) {
            if let None = inner.tasks[current].memory_set.translate(vpn) {
                return -1;
            };

            if let Some(pte) = inner.tasks[current].memory_set.translate(vpn) {
                if pte.is_valid() == false {
                    return -1;
                }
            };
        }

        for vpn in mm::VPNRange::new(mm::VirtPageNum::from(start_address), end_address.ceil()) {
            inner.tasks[current].memory_set.munmap(vpn);
        }

        for vpn in mm::VPNRange::new(mm::VirtPageNum::from(start_address), end_address.ceil()) {
            if let Some(pte) = inner.tasks[current].memory_set.translate(vpn) {
                if pte.is_valid() {
                    println!("[debug] This area is used!");
                    return -1;
                }
            };
        }

        return 0;
    }
}

/// Run the first task in task list.
pub fn run_first_task() {
    TASK_MANAGER.run_first_task();
}

/// Switch current `Running` task to the task we have found,
/// or there is no `Ready` task and we can exit with all applications completed
fn run_next_task() {
    TASK_MANAGER.run_next_task();
}

/// Change the status of current `Running` task into `Ready`.
fn mark_current_suspended() {
    TASK_MANAGER.mark_current_suspended();
}

/// Change the status of current `Running` task into `Exited`.
fn mark_current_exited() {
    TASK_MANAGER.mark_current_exited();
}

/// Suspend the current 'Running' task and run the next task in task list.
//挂起当前“正在运行”任务并运行任务列表中的下一个任务
pub fn suspend_current_and_run_next() {
    mark_current_suspended();
    run_next_task();
}

/// Exit the current 'Running' task and run the next task in task list.
pub fn exit_current_and_run_next() {
    mark_current_exited();
    run_next_task();
}

/// Get the current 'Running' task's token.
pub fn current_user_token() -> usize {
    TASK_MANAGER.get_current_token()
}

/// Get the current 'Running' task's trap contexts.
pub fn current_trap_cx() -> &'static mut TrapContext {
    TASK_MANAGER.get_current_trap_cx()
}

/// Get current task's time
pub fn get_current_task_time() -> usize {
    TASK_MANAGER.get_start_time() / 1000
}

/// Get all task's syscall times
pub fn get_syscall_times() -> [u32; 500] {
    TASK_MANAGER.get_syscall_times()
}

/// Update task's syscall times
pub fn update_syscall_times(id: usize) {
    TASK_MANAGER.update_syscall_times(id);
}

/// mmap
pub fn mmap(start: usize, len: usize, port: usize) -> isize {
    TASK_MANAGER.mmap(start, len, port)
}

/// munmap
pub fn munmap(start: usize, len: usize) -> isize {
    TASK_MANAGER.munmap(start, len)
}