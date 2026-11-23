//! Native child fixture, compiled once by the integration tests.
use std::{
    env,
    io::{self, Read, Write},
    process,
};

fn main() {
    let mut args = env::args_os().skip(1);
    match args.next().unwrap().to_str().unwrap() {
        "args" => {
            let mut out = io::stdout().lock();
            for arg in args {
                out.write_all(arg.as_encoded_bytes()).unwrap();
                out.write_all(&[0]).unwrap();
            }
            out.flush().unwrap();
        }
        "cwd" => println!("{}", env::current_dir().unwrap().display()),
        "env" => {
            let value = env::var_os(args.next().unwrap()).unwrap();
            io::stdout().write_all(value.as_encoded_bytes()).unwrap();
        }
        "status" => process::exit(args.next().unwrap().to_str().unwrap().parse().unwrap()),
        "both" => {
            print!("child stdout");
            io::stdout().flush().unwrap();
            eprint!("child stderr");
        }
        "stdin" => {
            let mut bytes = Vec::new();
            io::stdin().read_to_end(&mut bytes).unwrap();
            io::stdout().write_all(&bytes).unwrap();
            process::exit(7);
        }
        "copy" => {
            io::copy(&mut io::stdin().lock(), &mut io::stdout().lock()).unwrap();
        }
        "raw-stdin" => {
            let mut bytes = Vec::new();
            raw_standard_file(true).read_to_end(&mut bytes).unwrap();
        }
        "raw-stdout" => {
            raw_standard_file(false).write_all(b"raw output").unwrap();
        }
        "generate" => {
            let mut remaining: usize = args.next().unwrap().to_str().unwrap().parse().unwrap();
            let bytes: Vec<_> = (0..8192).map(|index| (index % 256) as u8).collect();
            let mut output = io::stdout().lock();
            while remaining > 0 {
                let count = remaining.min(bytes.len());
                if output.write_all(&bytes[..count]).is_err() { process::exit(1); }
                remaining -= count;
            }
        }
        "head" => {
            let count: u64 = args.next().unwrap().to_str().unwrap().parse().unwrap();
            io::copy(&mut io::stdin().take(count), &mut io::stdout().lock()).unwrap();
        }
        "delayed-mark" => {
            let path = args.next().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(250));
            std::fs::write(path, b"completed").unwrap();
        }
        "wait" => {
            println!("ready");
            io::stdout().flush().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        _ => process::exit(99),
    }
}

fn raw_standard_file(input: bool) -> std::fs::File {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        let handle = if input { io::stdin().as_handle().try_clone_to_owned() }
            else { io::stdout().as_handle().try_clone_to_owned() };
        handle.unwrap().into()
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        let descriptor = if input { io::stdin().as_fd().try_clone_to_owned() }
            else { io::stdout().as_fd().try_clone_to_owned() };
        descriptor.unwrap().into()
    }
}
