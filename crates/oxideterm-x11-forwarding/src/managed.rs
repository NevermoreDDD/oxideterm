// Copyright (C) 2026 AnalyseDeCircuit
// SPDX-License-Identifier: GPL-3.0-only

#[cfg(any(windows, test))]
use std::{ffi::OsString, path::Path};

#[cfg(any(windows, test))]
use zeroize::Zeroizing;

#[cfg(any(windows, test))]
use crate::{X11AuthCookie, X11AuthorityFamily, X11ForwardingError, X11Result};

#[cfg(any(windows, test))]
fn managed_launch_args(display: u16, authority_path: &Path, log_path: &Path) -> Vec<OsString> {
    [
        OsString::from(format!(":{display}")),
        OsString::from("-multiwindow"),
        OsString::from("-clipboard"),
        OsString::from("-primary"),
        OsString::from("-wgl"),
        OsString::from("-notrayicon"),
        OsString::from("-silent-dup-error"),
        OsString::from("-noreset"),
        OsString::from("-listen"),
        OsString::from("tcp"),
        OsString::from("-nolisten"),
        OsString::from("hyperv"),
        OsString::from("-auth"),
        authority_path.as_os_str().to_owned(),
        OsString::from("-logfile"),
        log_path.as_os_str().to_owned(),
        OsString::from("-logverbose"),
        OsString::from("1"),
    ]
    .into_iter()
    .collect()
}

#[cfg(any(windows, test))]
fn managed_authority_bytes(display: u16, cookie: &X11AuthCookie) -> X11Result<Zeroizing<Vec<u8>>> {
    let mut output = Zeroizing::new(Vec::new());
    output.extend_from_slice(&X11AuthorityFamily::Wild.code().to_be_bytes());
    push_authority_field(&mut output, &[])?;
    push_authority_field(&mut output, display.to_string().as_bytes())?;
    push_authority_field(&mut output, b"MIT-MAGIC-COOKIE-1")?;
    push_authority_field(&mut output, cookie.as_bytes())?;
    Ok(output)
}

#[cfg(any(windows, test))]
fn push_authority_field(output: &mut Vec<u8>, value: &[u8]) -> X11Result<()> {
    let len = u16::try_from(value.len()).map_err(|_| {
        X11ForwardingError::InvalidXauthRecord("managed xauthority field is too large")
    })?;
    output.extend_from_slice(&len.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

#[cfg(windows)]
mod windows_runtime {
    use std::{
        env, fs,
        net::{Ipv4Addr, SocketAddrV4, TcpListener},
        os::windows::process::CommandExt,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::{Arc, Mutex, OnceLock, Weak},
        time::{Duration, Instant},
    };

    use tempfile::TempDir;
    use tokio::{net::TcpStream, sync::Mutex as AsyncMutex, time::sleep};
    use windows::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject,
            },
            Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
        },
    };

    use crate::{
        X11AuthCommand, X11AuthMaterial, X11AuthorityFile, X11Display, X11ForwardConfig,
        X11ForwardPlan, X11ForwardPolicy, X11ForwardTrust, X11ForwardingError, X11LocalEndpoint,
        X11PreparedForwarding, X11Result, parse_xauth_list,
        prepare::{run_xauth_with_context, untrusted_generate_args, xauth_expiry_seconds},
    };

    use super::{managed_authority_bytes, managed_launch_args};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const MAX_MANAGED_DISPLAY: u16 = 63;
    const SERVER_START_TIMEOUT: Duration = Duration::from_secs(10);
    const SERVER_POLL_INTERVAL: Duration = Duration::from_millis(50);

    static MANAGED_RUNTIME: OnceLock<Mutex<Weak<ManagedWindowsX11Runtime>>> = OnceLock::new();

    pub struct ManagedWindowsX11Runtime {
        server: AsyncMutex<Option<Arc<ManagedWindowsXServer>>>,
    }

    impl ManagedWindowsX11Runtime {
        fn new() -> Self {
            Self {
                server: AsyncMutex::new(None),
            }
        }

        async fn prepare(&self, policy: X11ForwardPolicy) -> X11Result<X11PreparedForwarding> {
            let server = self.server().await?;
            server.prepare(policy).await
        }

        async fn server(&self) -> X11Result<Arc<ManagedWindowsXServer>> {
            let mut server = self.server.lock().await;
            if let Some(running) = server.as_ref()
                && endpoint_is_reachable(&running.endpoint).await
            {
                return Ok(Arc::clone(running));
            }

            *server = None;
            let running = Arc::new(ManagedWindowsXServer::start().await?);
            *server = Some(Arc::clone(&running));
            Ok(running)
        }
    }

    pub fn install_managed_windows_x11_runtime() -> Arc<ManagedWindowsX11Runtime> {
        let runtime = Arc::new(ManagedWindowsX11Runtime::new());
        let registry = MANAGED_RUNTIME.get_or_init(|| Mutex::new(Weak::new()));
        *registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::downgrade(&runtime);
        runtime
    }

    pub(crate) async fn prepare_managed_windows_x11_forwarding(
        policy: X11ForwardPolicy,
    ) -> X11Result<X11PreparedForwarding> {
        let runtime = MANAGED_RUNTIME
            .get()
            .and_then(|registry| {
                registry
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .upgrade()
            })
            .ok_or_else(|| {
                X11ForwardingError::ManagedRuntimeUnavailable(
                    "the application runtime owner is not installed".to_string(),
                )
            })?;
        runtime.prepare(policy).await
    }

    struct ManagedWindowsXServer {
        display: X11Display,
        endpoint: X11LocalEndpoint,
        runtime_dir: PathBuf,
        authority_path: PathBuf,
        cookie: crate::X11AuthCookie,
        child: Mutex<Child>,
        job: WindowsJob,
        _private_dir: TempDir,
    }

    impl ManagedWindowsXServer {
        async fn start() -> X11Result<Self> {
            let runtime_dir = find_runtime_dir()?;
            let private_dir = tempfile::Builder::new()
                .prefix("oxideterm-managed-x11-")
                .tempdir()
                .map_err(|error| {
                    X11ForwardingError::ManagedRuntimeFailed(format!(
                        "private runtime directory could not be created: {error}"
                    ))
                })?;
            let authority_path = private_dir.path().join("authority");
            let log_path = private_dir.path().join("vcxsrv.log");
            let cookie = crate::X11AuthCookie::random();
            let mut last_error = None;

            for display_number in 0..=MAX_MANAGED_DISPLAY {
                let port = 6000 + display_number;
                if !display_port_is_free(port) {
                    continue;
                }

                let authority = managed_authority_bytes(display_number, &cookie)?;
                fs::write(&authority_path, authority.as_slice()).map_err(|error| {
                    X11ForwardingError::ManagedRuntimeFailed(format!(
                        "private Xauthority could not be written: {error}"
                    ))
                })?;

                match Self::start_on_display(
                    display_number,
                    &runtime_dir,
                    &authority_path,
                    &log_path,
                )
                .await
                {
                    Ok((child, job)) => {
                        let display = X11Display::parse(&format!("127.0.0.1:{display_number}"))?;
                        let endpoint = display.local_endpoint()?;
                        return Ok(Self {
                            display,
                            endpoint,
                            runtime_dir,
                            authority_path,
                            cookie,
                            child: Mutex::new(child),
                            job,
                            _private_dir: private_dir,
                        });
                    }
                    Err(error) => last_error = Some(error),
                }
            }

            Err(last_error.unwrap_or_else(|| {
                X11ForwardingError::ManagedRuntimeFailed(
                    "no local X11 display port was available".to_string(),
                )
            }))
        }

        async fn start_on_display(
            display: u16,
            runtime_dir: &Path,
            authority_path: &Path,
            log_path: &Path,
        ) -> X11Result<(Child, WindowsJob)> {
            let executable = runtime_dir.join("vcxsrv.exe");
            let mut command = Command::new(&executable);
            command
                .args(managed_launch_args(display, authority_path, log_path))
                .current_dir(runtime_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW);
            let mut child = command.spawn().map_err(|error| {
                X11ForwardingError::ManagedRuntimeFailed(format!(
                    "bundled VcXsrv could not be started: {error}"
                ))
            })?;
            let job = match WindowsJob::new() {
                Ok(job) => job,
                Err(error) => {
                    terminate_child(&mut child);
                    return Err(error);
                }
            };
            if let Err(error) = job.assign_process(child.id()) {
                terminate_child(&mut child);
                return Err(error);
            }

            let endpoint = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6000 + display);
            if let Err(error) = wait_for_server(&mut child, endpoint).await {
                job.terminate();
                terminate_child(&mut child);
                return Err(error);
            }
            Ok((child, job))
        }

        async fn prepare(&self, policy: X11ForwardPolicy) -> X11Result<X11PreparedForwarding> {
            let config = X11ForwardConfig::new(self.display.clone()).with_policy(policy);
            let plan = match policy.trust {
                X11ForwardTrust::Trusted => X11ForwardPlan::new(
                    config,
                    X11AuthMaterial::mit_magic_cookie(self.cookie.clone()),
                ),
                X11ForwardTrust::Untrusted => self.prepare_untrusted_plan(config).await?,
            };
            Ok(X11PreparedForwarding {
                endpoint: self.endpoint.clone(),
                request: plan.ssh_request(),
                auth: plan.auth,
                acceptance_timeout: policy.timeout_millis.map(Duration::from_millis),
            })
        }

        async fn prepare_untrusted_plan(
            &self,
            config: X11ForwardConfig,
        ) -> X11Result<X11ForwardPlan> {
            let generated_dir = tempfile::Builder::new()
                .prefix("oxideterm-x11-untrusted-")
                .tempdir()
                .map_err(|error| X11ForwardingError::AuthorityFileUnavailable(error.to_string()))?;
            let generated_authority = generated_dir.path().join("authority");
            let xauth_program = self.runtime_dir.join("xauth.exe");
            run_xauth_with_context(
                X11AuthCommand {
                    program: xauth_program.to_string_lossy().into_owned(),
                    args: untrusted_generate_args(
                        &generated_authority,
                        &config.local_display,
                        xauth_expiry_seconds(config.policy.timeout_millis),
                    ),
                },
                Some(&self.runtime_dir),
                Some(&self.authority_path),
            )
            .await?;
            let output = run_xauth_with_context(
                X11AuthCommand {
                    program: xauth_program.to_string_lossy().into_owned(),
                    args: X11AuthCommand::list(
                        &config.local_display,
                        X11AuthorityFile::Path(generated_authority.to_string_lossy().into_owned()),
                    )
                    .args,
                },
                Some(&self.runtime_dir),
                Some(&self.authority_path),
            )
            .await?;
            let text = std::str::from_utf8(output.as_slice())
                .map_err(|error| X11ForwardingError::XauthFailed(error.to_string()))?;
            let plan = X11ForwardPlan::from_xauth_entries(config, &parse_xauth_list(text)?)?;
            drop(generated_dir);
            Ok(plan)
        }
    }

    impl Drop for ManagedWindowsXServer {
        fn drop(&mut self) {
            self.job.terminate();
            if let Ok(mut child) = self.child.lock() {
                terminate_child(&mut child);
            }
        }
    }

    fn find_runtime_dir() -> X11Result<PathBuf> {
        if let Some(runtime_dir) = env::var_os("OXIDETERM_X11_RUNTIME_DIR").map(PathBuf::from) {
            return validate_runtime_dir(runtime_dir);
        }

        let executable_dir = env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf));
        let mut candidates = Vec::new();
        if let Some(executable_dir) = executable_dir {
            candidates.push(executable_dir.join("resources/x11/vcxsrv"));
            candidates.push(executable_dir.join("x11/vcxsrv"));
        }
        if let Some(program_files) = env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(program_files).join("VcXsrv"));
        }

        candidates
            .into_iter()
            .find(|candidate| runtime_dir_is_complete(candidate))
            .ok_or_else(|| {
                X11ForwardingError::ManagedRuntimeUnavailable(
                    "the bundled VcXsrv files were not found".to_string(),
                )
            })
    }

    fn validate_runtime_dir(runtime_dir: PathBuf) -> X11Result<PathBuf> {
        runtime_dir_is_complete(&runtime_dir)
            .then_some(runtime_dir)
            .ok_or_else(|| {
                X11ForwardingError::ManagedRuntimeUnavailable(
                    "OXIDETERM_X11_RUNTIME_DIR does not contain a complete VcXsrv runtime"
                        .to_string(),
                )
            })
    }

    fn runtime_dir_is_complete(runtime_dir: &Path) -> bool {
        runtime_dir.join("vcxsrv.exe").is_file()
            && runtime_dir.join("xauth.exe").is_file()
            && runtime_dir.join("xkbdata").is_dir()
    }

    fn display_port_is_free(port: u16) -> bool {
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).is_ok()
    }

    async fn endpoint_is_reachable(endpoint: &X11LocalEndpoint) -> bool {
        let X11LocalEndpoint::Tcp { host, port } = endpoint else {
            return false;
        };
        tokio::time::timeout(
            Duration::from_millis(500),
            TcpStream::connect((host.as_str(), *port)),
        )
        .await
        .is_ok_and(|result| result.is_ok())
    }

    async fn wait_for_server(child: &mut Child, endpoint: SocketAddrV4) -> X11Result<()> {
        let deadline = Instant::now() + SERVER_START_TIMEOUT;
        loop {
            if TcpStream::connect(endpoint).await.is_ok() {
                return Ok(());
            }
            if let Some(status) = child.try_wait().map_err(|error| {
                X11ForwardingError::ManagedRuntimeFailed(format!(
                    "VcXsrv process status could not be read: {error}"
                ))
            })? {
                return Err(X11ForwardingError::ManagedRuntimeFailed(format!(
                    "VcXsrv exited during startup with {status}"
                )));
            }
            if Instant::now() >= deadline {
                return Err(X11ForwardingError::ManagedRuntimeFailed(
                    "VcXsrv did not accept X11 connections before the startup deadline".to_string(),
                ));
            }
            sleep(SERVER_POLL_INTERVAL).await;
        }
    }

    fn terminate_child(child: &mut Child) {
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
    }

    struct WindowsJob(HANDLE);

    // SAFETY: Windows job handles are process-wide kernel references and may be used from any thread.
    unsafe impl Send for WindowsJob {}
    // SAFETY: Every operation on the handle is performed by a thread-safe Win32 API.
    unsafe impl Sync for WindowsJob {}

    impl WindowsJob {
        fn new() -> X11Result<Self> {
            // SAFETY: The returned handle is owned by WindowsJob and closed exactly once in Drop.
            unsafe {
                let handle = CreateJobObjectW(None, None).map_err(|error| {
                    X11ForwardingError::ManagedRuntimeFailed(format!(
                        "X11 job object could not be created: {error}"
                    ))
                })?;
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if let Err(error) = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) {
                    let _ = CloseHandle(handle);
                    return Err(X11ForwardingError::ManagedRuntimeFailed(format!(
                        "X11 job object limits could not be set: {error}"
                    )));
                }
                Ok(Self(handle))
            }
        }

        fn assign_process(&self, process_id: u32) -> X11Result<()> {
            // SAFETY: The process handle is opened for this child PID and closed before returning.
            unsafe {
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, process_id)
                    .map_err(|error| {
                        X11ForwardingError::ManagedRuntimeFailed(format!(
                            "VcXsrv process could not be opened for lifecycle ownership: {error}"
                        ))
                    })?;
                let assignment = AssignProcessToJobObject(self.0, process).map_err(|error| {
                    X11ForwardingError::ManagedRuntimeFailed(format!(
                        "VcXsrv process could not be assigned to its job object: {error}"
                    ))
                });
                let _ = CloseHandle(process);
                assignment
            }
        }

        fn terminate(&self) {
            // SAFETY: The handle remains owned and valid until Drop closes it.
            let _ = unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for WindowsJob {
        fn drop(&mut self) {
            // SAFETY: WindowsJob uniquely owns this handle and Drop runs once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(windows)]
pub(crate) use windows_runtime::prepare_managed_windows_x11_forwarding;
#[cfg(windows)]
pub use windows_runtime::{ManagedWindowsX11Runtime, install_managed_windows_x11_runtime};

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{X11AuthCookie, X11AuthorityFamily, parse_xauthority_file};

    use super::{managed_authority_bytes, managed_launch_args};

    #[test]
    fn managed_server_keeps_access_control_and_uses_private_authority() {
        let args = managed_launch_args(
            7,
            Path::new(r"C:\Temp\OxideTerm\authority"),
            Path::new(r"C:\Temp\OxideTerm\vcxsrv.log"),
        );
        let args = args
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(args.first().map(String::as_str), Some(":7"));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-auth", r"C:\Temp\OxideTerm\authority"])
        );
        assert!(args.windows(2).any(|pair| pair == ["-listen", "tcp"]));
        assert!(args.windows(2).any(|pair| pair == ["-nolisten", "hyperv"]));
        assert!(args.iter().any(|argument| argument == "-multiwindow"));
        assert!(args.iter().any(|argument| argument == "-clipboard"));
        assert!(args.iter().any(|argument| argument == "-notrayicon"));
        assert!(!args.iter().any(|argument| argument == "-ac"));
    }

    #[test]
    fn managed_authority_is_a_redactable_wildcard_record() {
        let cookie = X11AuthCookie::from_hex("00112233445566778899aabbccddeeff").unwrap();

        let bytes = managed_authority_bytes(7, &cookie).unwrap();
        let entries = parse_xauthority_file(bytes.as_slice()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].family, X11AuthorityFamily::Wild);
        assert!(entries[0].address.is_empty());
        assert_eq!(entries[0].display_number, "7");
        assert_eq!(entries[0].cookie, cookie);
    }
}
