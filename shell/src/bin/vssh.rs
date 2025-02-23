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
        // Print the current directory
        let current_dir = env::current_dir().expect("Failed to get current directory");
        print!("\n{}$ ", current_dir.display());
        io::stdout().flush().expect("Failed to flush stdout");
        
        // Get user command and clean it up
        let mut input = String::new();
        io::stdin().read_line(&mut input).expect("Failed to read line");
        
        // Trim whitespaces, then parse words separated by whitespaces and then store into arguments
        let arguments: Vec<&str> = input.trim().split_whitespace().collect();
        
        // Get the command
        let command = arguments.first().unwrap_or(&"");
        
        match *command {
            "" => {
                continue;
            }
            "exit" => {
                break;
            }
            "cd" => {   
                shell_command(arguments);
            }
            _ => {
                external_command(arguments);
            }
        }
    }
}

// Checks if a present ampersand is at the end of the command
fn background_process(arguments: &mut Vec<&str>) -> i32 {
    if arguments.len() == 1 && arguments[0] == "&" {
        eprintln!("Error: syntax error near unexpected token `&'");
        return -1;
    }
    for i in 0..arguments.len() - 1 {
        if arguments[i] == "&" {
            eprintln!("Error: syntax error near unexpected token `&'");
            return -1;
        }
    }
    let last_arg: &str = arguments.last().unwrap_or(&"");
    if last_arg == "&" {
        arguments.pop();
        1
    } else {
        0
    }
}

// Checks for redirection symbols
fn verify_redirection(arguments: &[&str]) -> i32 {
    if arguments.first() == Some(&"<") || arguments.first() == Some(&">") {
        eprintln!("Error: syntax error near unexpected token `{}`", arguments.first().unwrap());
        return -1;
    }
    if arguments.last() == Some(&"<") || arguments.last() == Some(&">") {
        eprintln!("Error: syntax error near unexpected token `{}`", arguments.last().unwrap());
        return -1;
    }
    for i in 1..arguments.len() - 1 {
        if arguments[i] == "<" || arguments[i] == ">" {
            if i + 1 >= arguments.len() || arguments[i + 1] == "<" || arguments[i + 1] == ">" {
                eprintln!("Error: syntax error near unexpected token `{}`", arguments[i + 1]);
                return -1;
            }
            return 1;
        }
    }
    0
}

// Checks for pipeline symbols
fn verify_pipeline(arguments: &[&str]) -> i32 {
    if arguments.first() == Some(&"|") || arguments.last() == Some(&"|") {
        eprintln!("Error: syntax error near unexpected token `|'");
        return -1;
    }
    for i in 0..arguments.len() - 1 {
        if arguments[i] == "|" && arguments[i + 1] == "|" {
            eprintln!("Error: syntax error near unexpected token `||'");
            return -1;
        }
    }
    if arguments.contains(&"|") {
        return 1;
    }
    0
}

// Handles the cd command
fn shell_command(arguments: Vec<&str>) {
    if let Some(directory) = arguments.get(1) {
        let directory = directory.to_string();
        if let Err(e) = env::set_current_dir(directory) {
            eprintln!("{}", e);
        }
    } else {
        eprintln!("cd needs an argument");
    }
}

// Handles external commands
fn external_command(mut arguments: Vec<&str>) -> Option<String> {
    let background_flag = background_process(&mut arguments);
    let redirection_flag = verify_redirection(&arguments);
    let pipeline_flag = verify_pipeline(&arguments);

    if background_flag == -1 || redirection_flag == -1 || pipeline_flag == -1 {
        return None;
    }

    if pipeline_flag == 1 {
        if let Err(e) = handle_pipeline(arguments) {
            eprintln!("{}", e);
        }
        return None;
    }

    match unsafe { fork() } {
        Ok(ForkResult::Parent { child, .. }) => {
            if background_flag == 0 {
                waitpid(child, None).expect("Failed to wait on child");
            } else {
                println!("Starting background process {}", child);
            }
        }
        Ok(ForkResult::Child) => {
            if redirection_flag == 1 {
                if let Err(e) = handle_redirection(&mut arguments) {
                    eprintln!("{}", e);
                    unsafe { _exit(1); }
                }
            }

            let args = externalize(arguments);
            match execvp(&args[0], &args) {
                Ok(_) => unsafe { _exit(0); },
                Err(_) => {
                    eprintln!("{} not found", args[0].to_str().unwrap());
                    unsafe { _exit(1); }
                }
            }
        }
        Err(e) => {
            eprintln!("Fork failed: {}", e);
            return None;
        }
    }
    None
}

// Converts string slices to CStrings
fn externalize(arguments: Vec<&str>) -> Vec<CString> {
    arguments.into_iter()
        .map(|s| CString::new(s).unwrap())
        .collect()
}

// Handles redirection
fn handle_redirection(arguments: &mut Vec<&str>) -> Result<bool, String> {
    let mut i = 0;
    while i < arguments.len() {
        if arguments[i] == "<" || arguments[i] == ">" {
            let file_path = arguments.get(i + 1).ok_or(format!("Error: missing file path for redirection `{}`", arguments[i]))?;
            let file = match arguments[i] {
                ">" => File::create(file_path).map_err(|e| format!("Error: {}", e)),
                "<" => File::open(file_path).map_err(|e| format!("Error: {}", e)),
                _ => {
                    eprintln!("Error: unknown redirection symbol `{}`", arguments[i]);
                    return Ok(false);
                }
            }?;

            let fd = file.as_raw_fd();
            let std_fd = match arguments[i] {
                ">" => 1,
                "<" => 0,
                _ => return Err(format!("Error: unknown redirection symbol `{}`", arguments[i])),
            };

            unsafe {
                if libc::dup2(fd, std_fd) == -1 {
                    return Err(format!("Error: failed to duplicate file descriptor"));
                }
            }

            arguments.drain(i..=i + 1);
        } else {
            i += 1;
        }
    }
    Ok(true)
}

// Handles pipelines
fn handle_pipeline(args: Vec<&str>) -> Result<(), Error> {
    let mut commands: Vec<Vec<&str>> = Vec::new();
    let mut current_command: Vec<&str> = Vec::new();
    let mut input_file: Option<&str> = None;
    let mut output_file: Option<&str> = None;
    
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "|" => {
                if !current_command.is_empty() {
                    commands.push(current_command);
                    current_command = Vec::new();
                }
            }
            "<" => {
                if i + 1 < args.len() {
                    input_file = Some(args[i + 1]);
                    i += 1;
                }
            }
            ">" => {
                if i + 1 < args.len() {
                    output_file = Some(args[i + 1]);
                    i += 1;
                }
            }
            _ => {
                current_command.push(args[i]);
            }
        }
        i += 1;
    }

    if !current_command.is_empty() {
        commands.push(current_command);
    }

    let mut previous_stdout: Option<std::process::ChildStdout> = None;
    let commands_len = commands.len();

    for (i, command) in commands.iter().enumerate() {
        if command.is_empty() {
            continue;
        }

        let mut cmd = Command::new(command[0]);
        if command.len() > 1 {
            cmd.args(&command[1..]);
        }

        if let Some(prev_stdout) = previous_stdout.take() {
            cmd.stdin(Stdio::from(prev_stdout));
        } else if i == 0 && input_file.is_some() {
            let file = File::open(input_file.unwrap())?;
            cmd.stdin(Stdio::from(file));
        }

        if i == commands_len - 1 {
            if let Some(outfile) = output_file {
                let file = File::create(outfile)?;
                cmd.stdout(Stdio::from(file));
            } else {
                cmd.stdout(Stdio::inherit());
            }
        } else {
            cmd.stdout(Stdio::piped());
        }

        let mut child = cmd.spawn()?;

        if i != commands_len - 1 {
            previous_stdout = child.stdout.take();
        } else {
            child.wait()?;
        }
    }

    Ok(())
}