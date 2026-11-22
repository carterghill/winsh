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
        }
        "cwd" => println!("{}", env::current_dir().unwrap().display()),
        "env" => {
            let value = env::var_os(args.next().unwrap()).unwrap();
            io::stdout().write_all(value.as_encoded_bytes()).unwrap();
        }
        "status" => process::exit(args.next().unwrap().to_str().unwrap().parse().unwrap()),
        "both" => {
            print!("child stdout");
            eprint!("child stderr");
        }
        "stdin" => {
            let mut bytes = Vec::new();
            io::stdin().read_to_end(&mut bytes).unwrap();
            io::stdout().write_all(&bytes).unwrap();
            process::exit(7);
        }
        "wait" => {
            println!("ready");
            io::stdout().flush().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        _ => process::exit(99),
    }
}
