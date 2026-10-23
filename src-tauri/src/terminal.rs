//! Small, independently testable helpers for launching interactive CLI tools.

use std::path::Path;

/// Build a bash script for a terminal emulator on macOS.
///
/// Every path and argument is emitted as a single-quoted shell word. A failed
/// `cd` exits immediately; otherwise the script saves the CLI status before
/// showing the close prompt and returns that same status afterward.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn build_posix_script(
  bin: &Path,
  args: &[String],
  env: &[(&str, &str)],
  working_directory: &Path,
) -> String {
  let mut out = format!(
    "#!/bin/bash\ncd -- {} || exit $?\n",
    shell_quote(&working_directory.to_string_lossy()),
  );
  for (name, value) in env {
    if valid_env_name(name) {
      out.push_str(&format!("export {name}={}\n", shell_quote(value)));
    }
  }
  out.push_str(&shell_quote(&bin.to_string_lossy()));
  for arg in args {
    out.push(' ');
    out.push_str(&shell_quote(arg));
  }
  out.push_str(
    "\nstatus=$?\necho \"\"\nprintf 'Processo terminato (exit code: %s). Premi Invio per chiudere la finestra.\\n' \"$status\"\nread -r _\nexit \"$status\"\n",
  );
  out
}

#[cfg(any(target_os = "macos", test))]
fn shell_quote(value: &str) -> String {
  format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(any(target_os = "macos", test))]
fn valid_env_name(name: &str) -> bool {
  let mut chars = name.chars();
  matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
    && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Build a PowerShell 5.1 script. Data values are PowerShell single-quoted
/// literals, and arguments are passed as an array to the call operator.
#[cfg(any(windows, test))]
pub(crate) fn build_windows_script(
  bin: &Path,
  args: &[String],
  env: &[(&str, &str)],
  working_directory: &Path,
) -> String {
  let mut out = String::from(
    "$ErrorActionPreference = 'Stop'\n$exitCode = 1\ntry {\n  Set-Location -LiteralPath ",
  );
  out.push_str(&ps_quote(&windows_shell_path(working_directory)));
  out.push_str("\n");
  for (name, value) in env {
    out.push_str("  [Environment]::SetEnvironmentVariable(");
    out.push_str(&ps_quote(name));
    out.push_str(", ");
    out.push_str(&ps_quote(value));
    out.push_str(", 'Process')\n");
  }
  out.push_str("  $target = ");
  out.push_str(&ps_quote(&windows_shell_path(bin)));
  out.push_str("\n  $arguments = @(");
  for (index, arg) in args.iter().enumerate() {
    if index > 0 {
      out.push_str(", ");
    }
    out.push_str(&ps_quote(arg));
  }
  out.push_str(" )\n  $global:LASTEXITCODE = 0\n");
  if bin.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("ps1")) {
    out.push_str(
      r#"  function ConvertTo-WindowsCommandLineArgument([string]$value) {
    if ($value.Length -eq 0) { return '""' }
    $builder = New-Object System.Text.StringBuilder
    [void]$builder.Append('"')
    $slashes = 0
    foreach ($character in $value.ToCharArray()) {
      if ($character -eq '\') { $slashes++; continue }
      if ($character -eq '"') {
        [void]$builder.Append(('\' * (2 * $slashes + 1)))
        [void]$builder.Append('"')
      } else {
        [void]$builder.Append(('\' * $slashes))
        [void]$builder.Append($character)
      }
      $slashes = 0
    }
    [void]$builder.Append(('\' * (2 * $slashes)))
    [void]$builder.Append('"')
    return $builder.ToString()
  }
  $childArguments = @('-NoLogo', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $target) + $arguments
  $argumentLine = (($childArguments | ForEach-Object { ConvertTo-WindowsCommandLineArgument ([string]$_) }) -join ' ')
  $child = Start-Process -FilePath 'powershell.exe' -ArgumentList $argumentLine -WorkingDirectory (Get-Location).Path -PassThru -Wait -NoNewWindow
  $exitCode = $child.ExitCode
"#,
    );
  } else {
    out.push_str("  & $target @arguments\n  $exitCode = $LASTEXITCODE\n");
  }
  out.push_str(
    "} catch {\n  Write-Host (\"Errore durante l'avvio: {0}\" -f $_.Exception.Message) -ForegroundColor Red\n  $exitCode = 1\n}\ntry { Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue } catch {}\nWrite-Host ''\nWrite-Host (\"Processo terminato (exit code: {0}). Premi Invio per chiudere la finestra.\" -f $exitCode)\n[void](Read-Host)\nexit $exitCode\n",
  );
  out
}

/// Build the hidden PowerShell command which opens the visible terminal host.
/// The only value passed to Start-Process is the script path; Hive credentials
/// remain in the script file and never appear on a process command line.
#[cfg(any(windows, test))]
fn build_windows_bootstrap(powershell: &Path, script: &Path) -> String {
  let arguments = [
    "-NoLogo",
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
  ]
  .into_iter()
  .map(str::to_string)
  .chain(std::iter::once(windows_command_line_quote(&windows_shell_path(script))))
  .collect::<Vec<_>>()
  .join(" ");
  format!(
    "$ErrorActionPreference = 'Stop'; Start-Process -FilePath {} -ArgumentList {}",
    ps_quote(&powershell.to_string_lossy()),
    ps_quote(&arguments),
  )
}

#[cfg(any(windows, test))]
fn windows_command_line_quote(value: &str) -> String {
  let mut quoted = String::from("\"");
  let mut slashes = 0;
  for character in value.chars() {
    if character == '\\' {
      slashes += 1;
    } else if character == '"' {
      quoted.extend(std::iter::repeat('\\').take(slashes * 2 + 1));
      quoted.push('"');
      slashes = 0;
    } else {
      quoted.extend(std::iter::repeat('\\').take(slashes));
      quoted.push(character);
      slashes = 0;
    }
  }
  quoted.extend(std::iter::repeat('\\').take(slashes * 2));
  quoted.push('"');
  quoted
}

// canonicalize() produces verbatim Windows paths; PowerShell 5.1's location
// provider expects ordinary drive or UNC paths instead.
#[cfg(any(windows, test))]
fn windows_shell_path(path: &Path) -> String {
  let value = path.to_string_lossy();
  if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
    format!(r"\\{unc}")
  } else {
    value.strip_prefix(r"\\?\").unwrap_or(&value).to_string()
  }
}

#[cfg(any(windows, test))]
fn ps_quote(value: &str) -> String {
  format!("'{}'", value.replace('\'', "''"))
}

/// Open the script in a fresh console. `script_path` supplies the desired
/// directory and filename prefix; a process-unique suffix is added to avoid
/// collisions. PowerShell 5.1 requires a UTF-8 BOM to read non-ASCII paths.
#[cfg(windows)]
pub(crate) fn launch_windows(
  script_path: &Path,
  bin: &Path,
  args: &[String],
  env: &[(&str, &str)],
  working_directory: &Path,
) -> Result<(), String> {
  use std::os::windows::process::CommandExt;
  use std::process::{Command, Stdio};
  use std::sync::atomic::{AtomicU64, Ordering};

  static NEXT_SCRIPT_ID: AtomicU64 = AtomicU64::new(0);

  let parent = script_path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
  std::fs::create_dir_all(parent).map_err(|e| format!("Creazione cartella script non riuscita: {e}"))?;
  let stem = script_path.file_stem().and_then(|s| s.to_str()).unwrap_or("launch");
  let (path, file) = loop {
    let id = NEXT_SCRIPT_ID.fetch_add(1, Ordering::Relaxed);
    let path = parent.join(format!("{stem}-{}-{id}.ps1", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
      Ok(file) => break (path, file),
      Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
      Err(error) => return Err(format!("Creazione script di avvio non riuscita: {error}")),
    }
  };

  let mut file = file;
  let script = build_windows_script(bin, args, env, working_directory);
  let write_result = std::io::Write::write_all(&mut file, &[0xEF, 0xBB, 0xBF])
    .and_then(|()| std::io::Write::write_all(&mut file, script.as_bytes()));
  if let Err(error) = write_result {
    drop(file);
    let _ = std::fs::remove_file(&path);
    return Err(format!("Scrittura script di avvio non riuscita: {error}"));
  }
  drop(file);

  let script = match std::fs::canonicalize(&path) {
    Ok(script) => script,
    Err(error) => {
      let _ = std::fs::remove_file(&path);
      return Err(format!("Percorso dello script non valido: {error}"));
    }
  };
  let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
  let powershell = Path::new(&system_root)
    .join("System32")
    .join("WindowsPowerShell")
    .join("v1.0")
    .join("powershell.exe");
  let bootstrap = build_windows_bootstrap(&powershell, &script);
  let mut command = Command::new(&powershell);
  command
    .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
    .arg(bootstrap)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
  match command.output() {
    Ok(output) if output.status.success() => Ok(()),
    Ok(output) => {
      let _ = std::fs::remove_file(&path);
      Err(format!(
        "Impossibile aprire una nuova console interattiva (codice {}).",
        output.status.code().unwrap_or(1),
      ))
    }
    Err(error) => {
      let _ = std::fs::remove_file(&path);
      Err(format!("Impossibile avviare PowerShell: {error}"))
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn posix_script_quotes_values_aborts_failed_cd_and_keeps_exit_code() {
    let script = build_posix_script(
      Path::new("/opt/tool's/bin"),
      &["two words".into(), "$(touch /tmp/pwned)".into()],
      &[("TOKEN", "a'b;$HOME")],
      Path::new("/tmp/project's folder"),
    );
    assert!(script.contains("cd -- '/tmp/project'\\''s folder' || exit $?\n"));
    assert!(script.contains("export TOKEN='a'\\''b;$HOME'"));
    assert!(script.contains("'/opt/tool'\\''s/bin' 'two words' '$(touch /tmp/pwned)'"));
    assert!(script.contains("status=$?\n"));
    assert!(script.ends_with("read -r _\nexit \"$status\"\n"));
  }

  #[test]
  fn posix_script_omits_invalid_environment_names() {
    let script = build_posix_script(
      Path::new("tool"),
      &[],
      &[("OK_1", "x"), ("BAD;NAME", "y")],
      Path::new("."),
    );
    assert!(script.contains("export OK_1='x'"));
    assert!(!script.contains("BAD;NAME"));
  }

  #[cfg(unix)]
  #[test]
  fn posix_script_runs_literal_values_and_preserves_exit_status() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
      "requrv-terminal-{}-{}",
      std::process::id(),
      NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let cleanup = || {
      let _ = std::fs::remove_dir_all(&directory);
    };

    let marker = directory.join("marker");
    let env_marker = directory.join("env-marker");
    let executable = directory.join("fixture.sh");
    std::fs::write(
      &executable,
      format!(
        "#!/bin/bash\nprintf '%s\\n' \"$@\" > {}\nprintf '%s' \"$TOKEN\" > {}\nexit 7\n",
        shell_quote(&marker.to_string_lossy()),
        shell_quote(&env_marker.to_string_lossy()),
      ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

    let injected = "$(touch SHOULD_NOT_EXIST); 'quoted' $HOME";
    let script = build_posix_script(
      &executable,
      &[injected.to_string()],
      &[("TOKEN", injected)],
      &directory,
    );
    let script_path = directory.join("launch.sh");
    std::fs::write(&script_path, script).unwrap();
    let mut child = Command::new("bash")
      .arg(&script_path)
      .stdin(Stdio::piped())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .unwrap();
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    let status = child.wait().unwrap();

    assert_eq!(status.code(), Some(7));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), format!("{injected}\n"));
    assert_eq!(std::fs::read_to_string(&env_marker).unwrap(), injected);
    assert!(!directory.join("SHOULD_NOT_EXIST").exists());
    cleanup();
  }

  #[cfg(unix)]
  #[test]
  fn posix_script_does_not_run_command_when_working_directory_is_missing() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
      "requrv-terminal-cd-{}-{}",
      std::process::id(),
      NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let marker = directory.join("ran");
    let executable = directory.join("fixture.sh");
    std::fs::write(&executable, format!("#!/bin/bash\ntouch {}\n", shell_quote(&marker.to_string_lossy())))
      .unwrap();
    let script = build_posix_script(
      &executable,
      &[],
      &[],
      &directory.join("does-not-exist"),
    );
    let script_path = directory.join("launch.sh");
    std::fs::write(&script_path, script).unwrap();
    let mut child = Command::new("bash")
      .arg(&script_path)
      .stdin(Stdio::piped())
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .spawn()
      .unwrap();
    child.stdin.take().unwrap().write_all(b"\n").unwrap();
    let status = child.wait().unwrap();

    assert_ne!(status.code(), Some(0));
    assert!(!marker.exists());
    let _ = std::fs::remove_dir_all(&directory);
  }

  #[test]
  fn windows_verbatim_paths_are_compatible_with_powershell_locations() {
    assert_eq!(windows_shell_path(Path::new(r"\\?\C:\Users\工具\project")), r"C:\Users\工具\project");
    assert_eq!(windows_shell_path(Path::new(r"\\?\UNC\server\share\project")), r"\\server\share\project");
    let script = build_windows_script(
      Path::new(r"\\?\C:\bin\tool.exe"),
      &[],
      &[],
      Path::new(r"\\?\C:\Users\O'Neil\project"),
    );
    assert!(script.contains("Set-Location -LiteralPath 'C:\\Users\\O''Neil\\project'"));
    assert!(!script.contains(r"\\?\"));
  }

  #[test]
  fn windows_script_quotes_unicode_literals_without_interpolation() {
    let script = build_windows_script(
      Path::new(r"C:\Users\O'Neil\工具\tool.exe"),
      &["space here".into(), "$env:PATH; `x` & y".into()],
      &[("TOKEN", "a'b;$env:PATH")],
      Path::new(r"C:\Users\O'Neil\My Project"),
    );
    assert!(script.contains("Set-Location -LiteralPath 'C:\\Users\\O''Neil\\My Project'"));
    assert!(script.contains("'a''b;$env:PATH', 'Process'"));
    assert!(script.contains("'space here', '$env:PATH; `x` & y'"));
    assert!(script.contains("Write-Host (\"Errore durante l'avvio:"));
    assert!(!script.contains("Write-Error $_"));
    assert!(!script.contains("Add-Type"));
    assert!(script.contains("Read-Host"));
    assert!(script.contains("exit $exitCode"));

    let ps_script = build_windows_script(
      Path::new(r"C:\Users\O'Neil\tool.ps1"),
      &["argument with spaces".into()],
      &[],
      Path::new(r"C:\Users\O'Neil\My Project"),
    );
    assert!(ps_script.contains("Start-Process -FilePath 'powershell.exe'"));
    assert!(ps_script.contains("$child.ExitCode"));

    let bootstrap = build_windows_bootstrap(
      Path::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
      Path::new(r"C:\Temp\O'Neil & 工具\launch script.ps1"),
    );
    assert!(bootstrap.contains("Start-Process -FilePath 'C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe'"));
    assert!(bootstrap.contains("-File \"C:\\Temp\\O''Neil & 工具\\launch script.ps1\""));
    assert!(!bootstrap.contains("sk-secret"));
    assert_eq!(windows_command_line_quote(r"C:\folder with space\"), "\"C:\\folder with space\\\\\"");
  }

  #[cfg(windows)]
  #[test]
  fn windows_script_runs_exe_cmd_bat_and_ps1_with_exit_codes_and_unicode() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
      "requrv-terminal-工具-{}-{}",
      std::process::id(),
      NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let powershell = Path::new(&std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
      .join("System32")
      .join("WindowsPowerShell")
      .join("v1.0")
      .join("powershell.exe");

    let run_script = |name: &str, bin: &Path, args: &[String]| {
      let wrapper = directory.join(name);
      let script = build_windows_script(bin, args, &[], &directory);
      let mut file = std::fs::File::create(&wrapper).unwrap();
      file.write_all(&[0xEF, 0xBB, 0xBF]).unwrap();
      file.write_all(script.as_bytes()).unwrap();
      drop(file);
      let mut child = Command::new(&powershell)
        .args(["-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&wrapper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
      child.stdin.take().unwrap().write_all(b"\n").unwrap();
      child.wait_with_output().unwrap()
    };

    let exe_args = vec!["-NoLogo".into(), "-NoProfile".into(), "-Command".into(), "exit 7".into()];
    assert_eq!(run_script("exe-launch.ps1", &powershell, &exe_args).status.code(), Some(7));

    for extension in ["cmd", "bat"] {
      let shim = directory.join(format!("fixture.{extension}"));
      std::fs::write(&shim, "@echo off\r\nexit /b 7\r\n").unwrap();
      assert_eq!(run_script(&format!("{extension}-launch.ps1"), &shim, &[]).status.code(), Some(7));
    }

    let output_path = directory.join("unicode-output.txt");
    let ps1 = directory.join("fixture.ps1");
    std::fs::write(
      &ps1,
      "param([string]$path, [string]$value)\n[IO.File]::WriteAllText($path, $value)\nexit 7\n",
    )
    .unwrap();
    let unicode_arg = "città 東京 'quoted' & $env:PATH";
    let ps1_args = vec![output_path.to_string_lossy().into_owned(), unicode_arg.into()];
    assert_eq!(run_script("ps1-launch.ps1", &ps1, &ps1_args).status.code(), Some(7));
    assert_eq!(std::fs::read_to_string(&output_path).unwrap(), unicode_arg);
    let _ = std::fs::remove_dir_all(&directory);
  }
}
