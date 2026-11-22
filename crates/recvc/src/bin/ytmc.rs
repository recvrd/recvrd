//! Deprecated alias for the `recvc` binary.

use std::{
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

/// Return the sibling binary for `name`.
fn sibling_binary(current: &Path, name: &str) -> PathBuf {
    current.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

fn main() -> ExitCode {
    eprintln!("ytmc is deprecated; use recvc.");
    let current = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("could not find the current binary: {error}");
            return ExitCode::FAILURE;
        }
    };
    let target = sibling_binary(&current, "recvc");
    match Command::new(&target)
        .args(std::env::args_os().skip(1))
        .status()
    {
        Ok(status) => {
            let code = status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1);
            ExitCode::from(code)
        }
        Err(error) => {
            eprintln!("could not run {}: {error}", target.display());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_the_sibling_binary() {
        let target = sibling_binary(Path::new("/tmp/bin/ytmc"), "recvc");
        assert_eq!(
            target,
            PathBuf::from(format!("/tmp/bin/recvc{}", std::env::consts::EXE_SUFFIX))
        );
    }
}
