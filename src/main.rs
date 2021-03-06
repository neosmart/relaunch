extern crate getopts;
extern crate time;

use getopts::{Options, Matches};
use std::path::PathBuf;
use std::error::Error;

fn print_usage(opts: Options) {
    let brief = format!("{} TARGET [-- TARGET_OPTIONS]", opts.short_usage("relaunch"));
    print!("{}", opts.usage(&brief));
}

fn print_version() {
    println!("relaunch 0.1, Copyright NeoSmart Technologies 2017");
    println!("Developed by Mahmoud Al-Qudsi <mqudsi@neosmart.net>");
    println!("Licensed under the MIT open source license.");
}

fn main() {
    let mut args = Vec::<String>::new();
    let mut passthru_args = Vec::<String>::new();

    let mut separator_found = false;
    for arg in std::env::args().skip(1) {
        if arg == "--" {
            separator_found = true;
            continue;
        }
        if !separator_found {
            args.push(arg);
        }
        else {
            passthru_args.push(arg);
        }
    }

    let mut opts = Options::new();
    opts.optflag("a", "always-restart", "Always restart target, even on clean exit");
    // opts.optopt("j", "instances", "The number of instances of target to run in parallel", "N");
    opts.optopt("m", "max-restarts", "The maximum number of times to restart a process", "N");
    opts.optopt("i", "restart-interval", "Reset restart counter after SECS seconds", "SECS");
    opts.optopt("o", "stdout", "Redirect target stdout to PATH", "PATH");
    opts.optopt("e", "stderr", "Redirect target stderr to PATH", "PATH");
    opts.optopt("l", "log", "Path to relaunch output log", "PATH");
    opts.optflag("h", "help", "Print this help message and exit");
    opts.optflag("V", "version", "Print version info and exit");

    let matches = match opts.parse(&args) {
        Ok(m) => m,
        Err(e) =>  {
            println!("Error: {}", e);
            println!("relaunch --help provides usage information");
            std::process::exit(1);
        }
    };

    if matches.opt_present("h") {
        print_usage(opts);
        return;
    }
    if matches.opt_present("V") {
        print_version();
        return;
    }

    let mut moptions = MonitorOptions::new();
    // let mut loptions = LaunchOptions::new();

    // if matches.opt_present("j") {
    //     moptions.instances = unwrap_argument(&matches, "j", "-j/--instances requires a numeric value!");
    // }
    if matches.opt_present("m") {
        moptions.max_restarts = Some(unwrap_argument(&matches, "m", "-m/--max-restarts requires a numeric value!"));
    }
    if matches.opt_present("i") {
        moptions.restart_interval = Some(unwrap_argument(&matches, "i", "-i/--max-restart-interval requires a numeric value!"));
    }
    if matches.opt_present("o") {
        moptions.stdout = Some(unwrap_argument2(&matches, "o"));
    }
    if matches.opt_present("e") {
        moptions.stderr = Some(unwrap_argument2(&matches, "e"));
    }
    if matches.opt_present("l") {
        moptions.log = Some(unwrap_argument2(&matches, "l"));
    }
    if matches.opt_present("a") {
        moptions.restart_always = true;
    }

    if matches.free.len() != 1 {
        eprintln!("Error: TARGET must be specified and cannot include more than one command!");
        std::process::exit(1);
    }

    let target = &matches.free[0];

    let loptions = LaunchOptions {
        exe: target,
        args: passthru_args,
    };

    let mut logfile = None;

    //initialize logger
    if let Some(ref p) = moptions.log {
        logfile = match std::fs::OpenOptions::new().create(true).append(true).open(&p) {
            Err(e) => {
                eprintln!("Could not create log file: {}", e.description());
                std::process::exit(-1);
            },
            Ok(p) => Some(p)
        };
    };

    let logger = |s: &str| {
        println!("{}", s);

        if moptions.log.is_some() {
            use std::io::prelude::*;

            let prefix = format!("{} - ", time::now_utc().rfc3339());
            let mut bytes: Vec<u8> = prefix.bytes().collect();

            for b in s.bytes() {
                bytes.push(b);
            }
            bytes.push(b'\n');

            let mut logfile = match logfile {
                Some(ref l) => l,
                _ => panic!(),
            };
            if let Err(e) = logfile.write_all(&bytes) {
                eprintln!("Error writing to log file: {}!", e.description());
            }
        }
    };

    let exit_code = match relaunch(&loptions, &moptions, logger) {
        Ok(result) => match result {
            RelaunchResult::Ok => 0,
            RelaunchResult::OkAfterRestart(_) => 0,
            RelaunchResult::RestartCountExceeded(x) => x,
        },
        Err(err) => {
            let msg = match err {
                RelaunchError::LaunchErr(e) => format!("Error launching target: {}", e.description()),
                RelaunchError::StderrErr(e) => format!("Error redirecting stderr to file: {}", e.description()),
                RelaunchError::StdoutErr(e) => format!("Error redirecting stdout to file: {}", e.description()),
            };

            println!("{}", msg);
            -1
        }
    };

    std::process::exit(exit_code);
}

fn unwrap_argument<T>(matches: &Matches, arg: &'static str, msg: &'static str) -> T
    where T: std::str::FromStr
{
    match matches.opt_str(arg).unwrap().parse::<T>() {
        Ok(t) => t,
        Err(_) => {
            eprintln!("Error: {}", msg);
            std::process::exit(1);
        }
    }
}

fn unwrap_argument2<T>(matches: &Matches, arg: &'static str) -> T
    where T: std::convert::From<String>
{
    matches.opt_str(arg).unwrap().into()
}

fn relaunch<L>(loptions: &LaunchOptions, moptions: &MonitorOptions, mut logger: L) -> Result<RelaunchResult, RelaunchError>
    where L: FnMut(&str) -> ()
{
    use std::fs::OpenOptions;
    use std::process::Command;

    let mut fail_count = 0;
    let mut start_count = 0;
    let mut exit_code = None;

    loop {
        let mut cmd = Command::new(loptions.exe);
        cmd.args(&loptions.args);

        if let Some(ref path_stdout) = moptions.stdout {
            let stdout = OpenOptions::new().create(true).append(true).open(path_stdout).map_err(|e| RelaunchError::StdoutErr(e))?;
            cmd.stdout(stdout);
        }
        if let Some(ref path_stderr) = moptions.stderr {
            let stderr = OpenOptions::new().create(true).append(true).open(path_stderr).map_err(|e| RelaunchError::StderrErr(e))?;
            cmd.stderr(stderr);
        }

        let mut child = cmd.spawn().map_err(|e| RelaunchError::LaunchErr(e))?;
        logger(&format!("Monitoring new child process {} with pid {}", loptions.exe, child.id()));

        start_count += 1;
        let status = child.wait().unwrap();

        if !status.success() {
            logger(&format!("Child process {} exited {}", loptions.exe, match status.code() {
                Some(x) => format!("exit code {}", x),
                None => "due to signal!".to_owned(),
            }));
            fail_count += 1;
        }
        if status.success() {
            logger(&format!("Child process {} exited normally.", loptions.exe));
            if !moptions.restart_always {
                logger(&format!("Monitoring of process {} complete, exiting.", loptions.exe));
                break;
            }
        }

        //unix processes exited by a signal return no status code
        exit_code = status.code();

        let restart = match moptions.max_restarts {
            None => true,
            Some(x) => x > start_count - 1,
        };

        if !restart {
            logger(&format!("Max restart count exceeded, terminating relaunch of process {}", loptions.exe));
            break;
        }
    }

    match moptions.restart_always {
        true => Ok(RelaunchResult::RestartCountExceeded(fail_count)),
        false => match fail_count {
            0 => Ok(RelaunchResult::Ok),
            x => match exit_code {
                Some(0) => Ok(RelaunchResult::OkAfterRestart(x)),
                None | Some(_) => Ok(RelaunchResult::RestartCountExceeded(fail_count)),
            },
        }
    }
}

#[derive(Debug)]
struct LaunchOptions<'a> {
    exe: &'a str,
    args: Vec<String>,
}

#[derive(Debug)]
struct MonitorOptions {
    // instances: i32,
    max_restarts: Option<i32>,
    restart_always: bool,
    restart_interval: Option<i32>,
    // restart_codes: Option<Vec<i32>>,
    stdout: Option<PathBuf>,
    stderr: Option<PathBuf>,
    log: Option<PathBuf>,
}

impl MonitorOptions {
    fn new() -> Self {
        MonitorOptions {
            // instances: 1,
            max_restarts: Option::None,
            restart_always: false,
            restart_interval: Option::None,
            // restart_codes: Option::None,
            stdout: Option::None,
            stderr: Option::None,
            log: Option::None,
        }
    }
}

enum RelaunchResult {
    Ok, //never restarted, clean exit
    OkAfterRestart(i32), //restarted x times with clean exit
    RestartCountExceeded(i32), //attempts
}

enum RelaunchError {
    LaunchErr(std::io::Error),
    StdoutErr(std::io::Error),
    StderrErr(std::io::Error),
}
