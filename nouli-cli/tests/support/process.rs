use std::{
    process::{Child, Output},
    time::{Duration, Instant},
};
pub struct OwnedChild(Option<Child>);
impl OwnedChild {
    pub fn new(child: Child) -> Self {
        Self(Some(child))
    }
    pub fn stdout(&mut self) -> std::process::ChildStdout {
        self.0.as_mut().unwrap().stdout.take().unwrap()
    }
    pub fn id(&self) -> u32 {
        self.0.as_ref().unwrap().id()
    }
    pub fn output(mut self, timeout: Duration) -> Output {
        let deadline = Instant::now() + timeout;
        loop {
            if self.0.as_mut().unwrap().try_wait().unwrap().is_some() {
                return self.0.take().unwrap().wait_with_output().unwrap();
            }
            assert!(
                Instant::now() < deadline,
                "child did not exit before its bounded test deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
