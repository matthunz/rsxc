#![feature(rustc_private)]
#![feature(let_chains)]
#![feature(lazy_cell)]
#![feature(lint_reasons)]
#![cfg_attr(feature = "deny-warnings", deny(warnings))]
// warn on lints, that are included in `rust-lang/rust`s bootstrap
#![warn(rust_2018_idioms, unused_lifetimes)]
// warn on rustc internal lints
#![warn(rustc::internal)]

// FIXME: switch to something more ergonomic here, once available.
// (Currently there is no way to opt into sysroot crates without `extern crate`.)
extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_session;
extern crate rustc_span;

use rustc_interface::interface;
use rustc_session::config::ErrorOutputType;
use rustc_session::parse::ParseSess;
use rustc_session::EarlyErrorHandler;
use rustc_span::symbol::Symbol;
use std::env;
use std::ops::Deref;
use std::path::Path;
use std::process::exit;

/// If a command-line option matches `find_arg`, then apply the predicate `pred` on its value. If
/// true, then return it. The parameter is assumed to be either `--arg=value` or `--arg value`.
fn arg_value<'a, T: Deref<Target = str>>(
    args: &'a [T],
    find_arg: &str,
    pred: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    let mut args = args.iter().map(Deref::deref);
    while let Some(arg) = args.next() {
        let mut arg = arg.splitn(2, '=');
        if arg.next() != Some(find_arg) {
            continue;
        }

        match arg.next().or_else(|| args.next()) {
            Some(v) if pred(v) => return Some(v),
            _ => {}
        }
    }
    None
}

#[test]
fn test_arg_value() {
    let args = &["--bar=bar", "--foobar", "123", "--foo"];

    assert_eq!(arg_value(&[] as &[&str], "--foobar", |_| true), None);
    assert_eq!(arg_value(args, "--bar", |_| false), None);
    assert_eq!(arg_value(args, "--bar", |_| true), Some("bar"));
    assert_eq!(arg_value(args, "--bar", |p| p == "bar"), Some("bar"));
    assert_eq!(arg_value(args, "--bar", |p| p == "foo"), None);
    assert_eq!(arg_value(args, "--foobar", |p| p == "foo"), None);
    assert_eq!(arg_value(args, "--foobar", |p| p == "123"), Some("123"));
    assert_eq!(
        arg_value(args, "--foobar", |p| p.contains("12")),
        Some("123")
    );
    assert_eq!(arg_value(args, "--foo", |_| true), None);
}

fn track_clippy_args(parse_sess: &mut ParseSess, args_env_var: &Option<String>) {
    parse_sess.env_depinfo.get_mut().insert((
        Symbol::intern("CLIPPY_ARGS"),
        args_env_var.as_deref().map(Symbol::intern),
    ));
}

/// Track files that may be accessed at runtime in `file_depinfo` so that cargo will re-run clippy
/// when any of them are modified
fn track_files(parse_sess: &mut ParseSess) {
    let file_depinfo = parse_sess.file_depinfo.get_mut();

    // Used by `clippy::cargo` lints and to determine the MSRV. `cargo clippy` executes `clippy-driver`
    // with the current directory set to `CARGO_MANIFEST_DIR` so a relative path is fine
    if Path::new("Cargo.toml").exists() {
        file_depinfo.insert(Symbol::intern("Cargo.toml"));
    }

    // `clippy.toml` will be automatically tracked as it's loaded with `sess.source_map().load_file()`

    // During development track the `clippy-driver` executable so that cargo will re-run clippy whenever
    // it is rebuilt
    #[expect(
        clippy::collapsible_if,
        reason = "Due to a bug in let_chains this if statement can't be collapsed"
    )]
    if cfg!(debug_assertions) {
        if let Ok(current_exe) = env::current_exe()
            && let Some(current_exe) = current_exe.to_str()
        {
            file_depinfo.insert(Symbol::intern(current_exe));
        }
    }
}

struct DefaultCallbacks;
impl rustc_driver::Callbacks for DefaultCallbacks {}

/// This is different from `DefaultCallbacks` that it will inform Cargo to track the value of
/// `CLIPPY_ARGS` environment variable.
struct RustcCallbacks {
    clippy_args_var: Option<String>,
}

impl rustc_driver::Callbacks for RustcCallbacks {
    fn config(&mut self, config: &mut interface::Config) {
   
    }
}

struct ClippyCallbacks {
    clippy_args_var: Option<String>,
}

impl rustc_driver::Callbacks for ClippyCallbacks {
    fn config(&mut self, _config: &mut interface::Config) {}
}

pub fn main() {
    todo!();
    let handler = EarlyErrorHandler::new(ErrorOutputType::default());

    rustc_driver::init_rustc_env_logger(&handler);

    exit(rustc_driver::catch_with_exit_code(move || {
        let mut orig_args: Vec<String> = env::args().collect();
        let has_sysroot_arg = arg_value(&orig_args, "--sysroot", |_| true).is_some();

        let sys_root_env = std::env::var("SYSROOT").ok();
        let pass_sysroot_env_if_given =
            |args: &mut Vec<String>, sys_root_env| {
                if let Some(sys_root) = sys_root_env {
                    if !has_sysroot_arg {
                        args.extend(vec!["--sysroot".into(), sys_root]);
                    }
                };
            };

        // make "clippy-driver --rustc" work like a subcommand that passes further args to "rustc"
        // for example `clippy-driver --rustc --version` will print the rustc version that clippy-driver
        // uses
        if let Some(pos) = orig_args.iter().position(|arg| arg == "--rustc") {
            orig_args.remove(pos);
            orig_args[0] = "rustc".to_string();

            let mut args: Vec<String> = orig_args.clone();
            pass_sysroot_env_if_given(&mut args, sys_root_env);

            return rustc_driver::RunCompiler::new(&args, &mut DefaultCallbacks).run();
        }

        if orig_args.iter().any(|a| a == "--version" || a == "-V") {
            exit(0);
        }

        // Setting RUSTC_WRAPPER causes Cargo to pass 'rustc' as the first argument.
        // We're invoking the compiler programmatically, so we ignore this/
        let wrapper_mode =
            orig_args.get(1).map(Path::new).and_then(Path::file_stem) == Some("rustc".as_ref());

        if wrapper_mode {
            // we still want to be able to invoke it normally though
            orig_args.remove(1);
        }

        if !wrapper_mode
            && (orig_args.iter().any(|a| a == "--help" || a == "-h") || orig_args.len() == 1)
        {
            exit(0);
        }

        let mut args: Vec<String> = orig_args.clone();
        pass_sysroot_env_if_given(&mut args, sys_root_env);

        let mut no_deps = false;
        let clippy_args_var = env::var("CLIPPY_ARGS").ok();
     
        Ok(())
    }))
}
