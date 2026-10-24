//! One owned foreground Ctrl-C observer for explicit CLI invocations.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

pub struct InterruptWatch {
    done: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl InterruptWatch {
    pub fn start(cancelled: Arc<AtomicBool>) -> Result<Self, String> {
        let observer = minicon_platform::console_interrupt::ConsoleInterruptObserver::install()
            .map_err(|_| "foreground interrupt observer unavailable".to_owned())?;
        let done = Arc::new(AtomicBool::new(false));
        let stopping = done.clone();
        let worker = std::thread::Builder::new()
            .name("minicon-cli-interrupt".into())
            .spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    match observer.take_pending() {
                        Ok(false) => {}
                        Ok(true) | Err(_) => {
                            cancelled.store(true, Ordering::Release);
                            break;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            })
            .map_err(|_| "foreground interrupt worker unavailable".to_owned())?;
        Ok(Self {
            done,
            worker: Some(worker),
        })
    }
}

impl Drop for InterruptWatch {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
