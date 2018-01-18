use core::{VerifierCore, CoreRef};
use errors::*;
use error_report;
use num_cpus;
use parking_lot::Mutex;
use std::boxed::FnBox;
use std::mem::{uninitialized, drop};
use std::ptr;
use std::sync::Arc;
use std::thread;
use std::thread::Builder;
use std::time::Duration;
use threadpool::ThreadPool;

const MAX_SECS: usize = 4096; // 68 minutes

enum Task {
    NormalTask(Box<FnBox(&VerifierCore) -> Result<()> + Send + 'static>),
    RepeatingTask(Arc<Fn(&VerifierCore) -> Result<()> + Sync + Send + 'static>, usize),
}
struct TaskList {
    task: Task, next: Option<Box<TaskList>>,
}
struct TimerRing {
    slots: [Option<Box<TaskList>>; MAX_SECS], cur_pos: usize,
}
struct TaskManagerData {
    core_ref: CoreRef, pool: Mutex<ThreadPool>, timer_ring: Mutex<TimerRing>,
}

#[derive(Clone)]
pub struct TaskManager(Arc<TaskManagerData>);
impl TaskManager {
    pub(in ::core) fn new(core_ref: CoreRef) -> Result<TaskManager> {
        let slots = unsafe {
            let mut slots: [Option<Box<TaskList>>; MAX_SECS] = uninitialized();
            for slot in slots.iter_mut() {
                ptr::write(slot, None)
            }
            slots
        };
        let tasks = TaskManager(Arc::new(TaskManagerData {
            core_ref,
            pool: Mutex::new(ThreadPool::with_name("task thread".to_string(), num_cpus::get())),
            timer_ring: Mutex::new(TimerRing { slots, cur_pos: 0, }),
        }));
        {
            let task_data = Arc::downgrade(&tasks.0);
            Builder::new().name("timer thread".to_string()).spawn(move || {
                loop {
                    if let Some(task_data) = task_data.upgrade() {
                        let mut timer_ring = task_data.timer_ring.lock();
                        let cur_pos = timer_ring.cur_pos;
                        let mut slot = timer_ring.slots[cur_pos].take();
                        timer_ring.cur_pos = (timer_ring.cur_pos + 1) % MAX_SECS;
                        drop(timer_ring);

                        let tasks = TaskManager(task_data);
                        while let Some(cur_slot) = slot {
                            match cur_slot.task {
                                Task::NormalTask(task) =>
                                    tasks.dispatch_task(|core| FnBox::call_box(task, (core,))),
                                Task::RepeatingTask(task, period_secs) => {
                                    {
                                        let task = task.clone();
                                        tasks.dispatch_task(move |core| task(core));
                                    }
                                    tasks.push_to_ring(Task::RepeatingTask(task, period_secs),
                                                       period_secs);
                                }
                            }
                            slot = cur_slot.next;
                        }
                    } else {
                        break
                    }
                    thread::sleep(Duration::from_secs(1));
                }
            })?;
        }
        Ok(tasks)
    }

    pub fn dispatch_task<F>(
        &self, f: F
    ) where F: FnOnce(&VerifierCore) -> Result<()> + Send + 'static {
        let core_ref = self.0.core_ref.clone();
        self.0.pool.lock().execute(move || {
            error_report::catch_error(|| {
                if let Some(core) = core_ref.get_core() {
                    f(&core)
                } else {
                    Ok(())
                }
            }).ok();
        })
    }

    fn push_to_ring(&self, task: Task, duration_secs: usize) {
        let mut ring = self.0.timer_ring.lock();
        let target_slot = (ring.cur_pos + duration_secs - 1) % MAX_SECS;
        let next = ring.slots[target_slot].take();
        ring.slots[target_slot] = Some(Box::new(TaskList { task, next }));
    }
    pub fn dispatch_delayed_task<F>(
        &self, wait: Duration, f: F
    ) where F: FnOnce(&VerifierCore) -> Result<()> + Send + 'static {
        let duration_secs = wait.as_secs();
        assert!(duration_secs as usize <= MAX_SECS);
        let f = Box::new(f);
        if duration_secs == 0 {
            self.dispatch_task(move |core| FnBox::call_box(f, (core,)));
        } else {
            self.push_to_ring(Task::NormalTask(f), duration_secs as usize);
        }
    }
    pub fn dispatch_repeating_task<F>(
        &self, period: Duration, f: F
    ) where F: Fn(&VerifierCore) -> Result<()> + Send + Sync + 'static {
        let period_secs = period.as_secs();
        assert!(period_secs > 0 && period_secs as usize <= MAX_SECS);
        self.push_to_ring(Task::RepeatingTask(Arc::new(f), period_secs as usize),
                          period_secs as usize);
    }
}