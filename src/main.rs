//! 周期规则裁决器 —— 本地服务入口。

use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex};

use tiny_http::Server;

use periodic_arbiter::api::AppState;
use periodic_arbiter::server;
use periodic_arbiter::storage::Store;

#[derive(Debug)]
struct Args {
    addr: String,
    data_dir: String,
}

fn parse_args() -> Args {
    let mut args = std::env::args().skip(1);
    let mut addr = "127.0.0.1:5217".to_string();
    let mut data_dir = ".periodic-arbiter-data".to_string();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--addr" => {
                if let Some(v) = args.next() {
                    addr = v;
                }
            }
            "--data-dir" => {
                if let Some(v) = args.next() {
                    data_dir = v;
                }
            }
            "-h" | "--help" => {
                println!(
                    "周期规则裁决器\n\nUSAGE:\n    periodic-arbiter [--addr 127.0.0.1:5217] [--data-dir DIR]\n"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("未知参数: {other}（用 --help 查看用法）");
                std::process::exit(2);
            }
        }
    }
    Args { addr, data_dir }
}

fn main() {
    let args = parse_args();
    let mut addrs = args
        .addr
        .to_socket_addrs()
        .unwrap_or_else(|e| panic!("无法解析地址 {}: {e}", args.addr));
    let addr = addrs
        .next()
        .unwrap_or_else(|| panic!("地址 {} 没有可用的 socket 地址", args.addr));

    let store = Store::from_env(&args.data_dir)
        .unwrap_or_else(|e| panic!("无法打开数据目录 {}: {e}", args.data_dir));
    println!(
        "周期规则裁决器：数据目录 {}",
        store.root().to_string_lossy()
    );
    println!("内置时区数据库: {}", periodic_arbiter::tzutil::tzdb_version());

    let state = Arc::new(Mutex::new(AppState { store }));
    let server = Server::http(addr).unwrap_or_else(|e| panic!("无法在 {addr} 监听: {e}"));
    println!("服务已启动: http://{}", args.addr);
    server::serve(server, state);
}
