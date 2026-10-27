//! Cooperative cancellation with a commit boundary for filesystem operations.
use crate::Result;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, AtomicU32, Ordering},
    mpsc,
};
#[derive(Clone)]
pub struct Control {
    state: Arc<AtomicU8>,
    progress: Arc<AtomicU32>,
    stage: Arc<Mutex<String>>,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(0)),
            progress: Arc::new(AtomicU32::new(0)),
            stage: Arc::new(Mutex::new(String::new())),
        }
    }
}
impl Control {
    pub fn cancel(&self) -> bool {
        self.state
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
    pub fn checkpoint(&self, percent: u32, stage: &str) -> Result<()> {
        if self.state.load(Ordering::SeqCst) == 1 {
            return Err("処理を取り消しました".into());
        }
        self.progress.store(percent.min(100), Ordering::Relaxed);
        *self.stage.lock().unwrap() = stage.into();
        Ok(())
    }
    pub fn begin_commit(&self) -> Result<()> {
        self.state
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .map_err(|_| "処理を取り消しました".into())
    }
    pub fn status(&self) -> (u32, String) {
        (
            self.progress.load(Ordering::Relaxed),
            self.stage.lock().unwrap().clone(),
        )
    }
    pub fn cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == 1
    }
}
pub struct Task<T> {
    pub control: Control,
    receiver: mpsc::Receiver<Result<T>>,
}
impl<T: Send + 'static> Task<T> {
    pub fn spawn(work: impl FnOnce(Control) -> Result<T> + Send + 'static) -> Self {
        let control = Control::default();
        let c = control.clone();
        let (tx, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(c)))
                .unwrap_or_else(|_| Err("バックグラウンド処理が失敗しました".into()));
            let _ = tx.send(result);
        });
        Self { control, receiver }
    }
    pub fn poll(&self) -> Option<Result<T>> {
        match self.receiver.try_recv() {
            Ok(v) => Some(v),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err("処理が終了しました".into())),
        }
    }
}
impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.control.cancel();
    }
}
