use std::process::ExitCode;

use helm_service::cli::{self, Command};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(cmd) => match platform::execute(cmd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("helm-service: {e}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("helm-service: {e}\n\n{}", cli::USAGE);
            ExitCode::from(2)
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::path::PathBuf;

    use helm_service::AGENT_EXE;
    use helm_service::cli::Command;
    use helm_service::win::{scm, service};

    fn default_agent(agent: Option<PathBuf>) -> Result<PathBuf, String> {
        if let Some(a) = agent {
            return std::path::absolute(&a).map_err(|e| e.to_string());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        Ok(exe.with_file_name(AGENT_EXE))
    }

    pub fn execute(cmd: Command) -> Result<(), String> {
        match cmd {
            Command::Install {
                agent,
                start,
                agent_args,
            } => scm::install(&default_agent(agent)?, start, &agent_args),
            Command::Uninstall => scm::uninstall(),
            Command::Start => scm::start(),
            Command::Stop => scm::stop(),
            Command::Status => scm::status(),
            Command::Run { agent, agent_args } => {
                let config = service::RunConfig {
                    agent: default_agent(agent)?,
                    agent_args,
                };
                service::run_dispatcher(config).map_err(|e| {
                    format!("{e}\n`run` is started by the Service Control Manager; use `install` then `start`.")
                })
            }
            Command::Help => Ok(()),
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use helm_service::cli::Command;

    pub fn execute(_cmd: Command) -> Result<(), String> {
        Err("helm-service only runs on Windows".into())
    }
}
