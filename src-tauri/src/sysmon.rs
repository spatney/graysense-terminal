//! Lightweight system monitor (sampled every 2 s on a background thread).

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Clone, Default)]
pub struct SysStats {
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub app_mem: u64,
    pub cpu_hist: Vec<f32>,
}

pub fn spawn() -> Arc<Mutex<SysStats>> {
    let stats = Arc::new(Mutex::new(SysStats { cpu_hist: vec![0.0; 30], ..Default::default() }));
    let out = stats.clone();
    std::thread::Builder::new()
        .name("graysense-sysmon".into())
        .spawn(move || {
            let mut sys = System::new();
            let pid = Pid::from_u32(std::process::id());
            loop {
                sys.refresh_cpu_usage();
                sys.refresh_memory();
                sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, ProcessRefreshKind::nothing().with_memory());
                let app_mem = sys.process(pid).map(|p| p.memory()).unwrap_or(0);
                {
                    let mut s = out.lock();
                    s.cpu = sys.global_cpu_usage();
                    s.mem_used = sys.used_memory();
                    s.mem_total = sys.total_memory();
                    s.app_mem = app_mem;
                    let cpu = s.cpu;
                    s.cpu_hist.remove(0);
                    s.cpu_hist.push(cpu);
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .ok();
    stats
}
