use std::{env, process};

use pysub_compiler::{compile_file, CompilerError};

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("Usage: pysub-compiler <source-file>");
        process::exit(1);
    };

    if let Some(extra) = args.next() {
        eprintln!("unexpected extra argument: {extra}");
        process::exit(1);
    }

    match compile_file(&path) {
        Ok(_) => println!("Compiled {path}"),
        Err(err) => report_error(err),
    }
}

fn report_error(err: CompilerError) -> ! {
    eprintln!("error: {err}");
    process::exit(1);
}
