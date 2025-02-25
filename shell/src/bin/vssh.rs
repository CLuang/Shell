use std::env;
use std::io::{self, Write};
use nix::unistd::{fork, ForkResult, execvp};
use nix::libc::{self, _exit};
use nix::sys::wait::waitpid;
use std::ffi::CString;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::process::{Command, Stdio};
use std::io::Error;

fn main() {
    loop {
        print_prompt();
        let input = read_input();
        let mut arguments = parse_input(&input);
        
        if arguments.is_empty() {
            continue;
        }

        match arguments[0] {
            "exit" => break,
            "cd" => handle_cd(&arguments),
            _ => execute_command(arguments),
        }
    }
}

fn print_prompt() {
    let current_dir = env::current_dir().unwrap_or_else(|_| env::temp_dir());
    print!("\n{}$ ", current_dir.display());
    io::stdout().flush().expect("Failed to flush stdout");
}

fn read_input() -> String {
    let mut input = String::new();
    io::stdin().read_line(&mut input).expect("Failed to read input");
    input
}

fn parse_input(input: &str) -> Vec<&str> {
    input.trim().split_whitespace().collect()
}

fn handle_cd(arguments: &[&str]) {
    if let Some(dir) = arguments.get(1) {
        if let Err(e) = env::set_current_dir(dir) {
            eprintln!("Error: {}", e);
        }
    } else {
        eprintln!("cd requires an argument");
    }
}

fn execute_command(mut arguments: Vec<&str>) {
    let background = check_background(&mut arguments);
    let redirection = check_redirection(&arguments);
    let pipeline = check_pipeline(&arguments);

    if background == -1 || redirection == -1 || pipeline == -1 {
        return;
    }

    if pipeline == 1 {
        if let Err(e) = handle_pipeline(arguments) {
            eprintln!("{}", e);
        }
        return;
    }

    match unsafe { fork() } {
        Ok(ForkResult::Parent { child, .. }) => {
            if background == 0 {
                waitpid(child, None).expect("Failed to wait on child");
            } else {
                println!("Started background process: {}", child);
            }
        }
        Ok(ForkResult::Child) => {
            if redirection == 1 {
                if let Err(e) = handle_redirection(&mut arguments) {
                    eprintln!("{}", e);
                    unsafe { _exit(1); }
                }
            }
            let args = externalize(arguments);
            if execvp(&args[0], &args).is_err() {
                eprintln!("Command not found: {}", args[0].to_str().unwrap());
                unsafe { _exit(1); }
            }
        }
        Err(e) => eprintln!("Fork failed: {}", e),
    }
}

fn check_background(arguments: &mut Vec<&str>) -> i32 {
    if arguments.last() == Some(&"&") {
        arguments.pop();
        return 1;
    }
    0
}

fn check_redirection(arguments: &[&str]) -> i32 {
    if arguments.iter().any(|&arg| arg == "<" || arg == ">") {
        1
    } else {
        0
    }
}

fn check_pipeline(arguments: &[&str]) -> i32 {
    if arguments.contains(&"|") { 1 } else { 0 }
}

fn externalize(arguments: Vec<&str>) -> Vec<CString> {
    arguments.into_iter().map(|s| CString::new(s).unwrap()).collect()
}

fn handle_redirection(arguments: &mut Vec<&str>) -> Result<bool, String> {
    let mut i = 0;
    while i < arguments.len() {
        if arguments[i] == "<" || arguments[i] == ">" {
            let file = arguments.get(i + 1).ok_or("Missing file for redirection")?;
            let fd = match arguments[i] {
                ">" => File::create(file).map_err(|e| e.to_string())?.as_raw_fd(),
                "<" => File::open(file).map_err(|e| e.to_string())?.as_raw_fd(),
                _ => return Err("Invalid redirection symbol".into()),
            };
            let std_fd = if arguments[i] == ">" { 1 } else { 0 };
            unsafe { libc::dup2(fd, std_fd) };
            arguments.drain(i..=i + 1);
        } else {
            i += 1;
        }
    }
    Ok(true)
}

fn handle_pipeline(args: Vec<&str>) -> Result<(), Error> {
    let commands: Vec<Vec<&str>> = args.split(|&arg| arg == "|").map(|cmd| cmd.to_vec()).collect();
    let mut previous_stdout: Option<std::process::ChildStdout> = None;
    
    for (i, command) in commands.iter().enumerate() {
        let mut cmd = Command::new(command[0]);
        if command.len() > 1 {
            cmd.args(&command[1..]);
        }
        if let Some(prev_stdout) = previous_stdout.take() {
            cmd.stdin(Stdio::from(prev_stdout));
        }
        cmd.stdout(if i == commands.len() - 1 { Stdio::inherit() } else { Stdio::piped() });
        let mut child = cmd.spawn()?;
        if i != commands.len() - 1 {
            previous_stdout = child.stdout.take();
        } else {
            child.wait()?;
        }
    }
    Ok(())
}
