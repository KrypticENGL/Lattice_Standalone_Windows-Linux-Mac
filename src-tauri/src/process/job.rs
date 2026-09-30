//! Process-tree termination. On Windows this is a Job Object with
//! KILL_ON_JOB_CLOSE, so a runaway program (and anything it spawned) dies with
//! it, even if Lattice itself is killed.
//!
//! Known gap: the child is assigned to the job just after `spawn`, so a
//! grandchild spawned in that first instant could escape. Acceptable for
//! runaway-loop protection; not a security boundary.

use std::process::Child;

#[cfg(windows)]
pub struct ProcessTree(Option<windows_sys::Win32::Foundation::HANDLE>);

#[cfg(windows)]
unsafe impl Send for ProcessTree {}

#[cfg(windows)]
impl ProcessTree {
    pub fn attach(child: &Child) -> Self {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::*;

        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                tracing::warn!("CreateJobObjectW failed; falling back to single-process kill");
                return Self(None);
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
                tracing::warn!("could not attach process to job object; falling back to single-process kill");
                CloseHandle(job);
                return Self(None);
            }
            Self(Some(job))
        }
    }

    pub fn terminate(&self, child: &mut Child) {
        match self.0 {
            Some(job) => unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(job, 1);
            },
            None => {
                let _ = child.kill();
            }
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        if let Some(job) = self.0 {
            // KILL_ON_JOB_CLOSE reaps any stragglers.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
        }
    }
}

#[cfg(not(windows))]
pub struct ProcessTree;

#[cfg(not(windows))]
impl ProcessTree {
    pub fn attach(_child: &Child) -> Self {
        Self
    }
    pub fn terminate(&self, child: &mut Child) {
        let _ = child.kill();
    }
}
