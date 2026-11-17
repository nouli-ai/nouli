use std::{
    process::{Child, Output},
    time::{Duration, Instant},
};
#[path = "child.rs"]
pub mod fixture;
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
    pub fn kill(&mut self) {
        self.0.as_mut().unwrap().kill().unwrap();
    }
    pub fn interrupt(&self) {
        let mut command = fixture::command("interrupt");
        command.arg(self.id().to_string());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        assert!(
            command.status().unwrap().success(),
            "could not signal isolated CLI console"
        );
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

pub fn prepare_interrupt(command: &mut std::process::Command) {
    #[cfg(unix)]
    let _ = command;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut isolated = fixture::command("console");
        isolated
            .arg(command.get_program())
            .args(command.get_args())
            .env_clear();
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                isolated.env(key, value);
            }
        }
        if let Some(directory) = command.get_current_dir() {
            isolated.current_dir(directory);
        }
        isolated
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        *command = isolated;
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
