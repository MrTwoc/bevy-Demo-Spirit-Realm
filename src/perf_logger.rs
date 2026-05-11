use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bevy::{
    diagnostic::{DiagnosticPath, DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
    time::Timer,
};

use crate::chunk_manager::LoadedChunks;

/// 性能记录器的配置资源。
#[derive(Resource)]
pub struct PerfLoggerConfig {
    /// 是否启用性能记录
    pub enabled: bool,
    /// 记录间隔（秒）
    pub interval_secs: f32,
    /// 输出目录（相对于工作目录）
    pub output_dir: PathBuf,
}

impl Default for PerfLoggerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 1.0,
            output_dir: PathBuf::from("perf_logs"),
        }
    }
}

/// 性能记录器的内部状态。
#[derive(Resource)]
struct PerfLoggerState {
    writer: BufWriter<File>,
    timer: Timer,
    start_time: Instant,
    frame_count: u64,
    /// 是否已打印诊断路径（首次记录时打印一次用于调试）
    diagnostics_printed: bool,
    /// 自动发现的三角面诊断路径
    triangle_paths: Vec<DiagnosticPath>,
}

/// 性能记录插件。
pub struct PerfLoggerPlugin;

impl Plugin for PerfLoggerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PerfLoggerConfig>()
            .add_systems(Startup, init_perf_logger)
            .add_systems(Update, record_perf_metrics);
    }
}

/// 生成时间戳文件名，格式：`YYYY-MM-DD_HH-MM-SS-perf_log.csv`
fn generate_timestamp_filename() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // 将 Unix 时间戳转换为年月日时分秒（简化版，不依赖外部库）
    let (year, month, day, hour, minute, second) = unix_to_ymdhms(now);

    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}-perf_log.csv",
        year, month, day, hour, minute, second
    )
}

/// 将 Unix 时间戳（秒）转换为 (年, 月, 日, 时, 分, 秒)，使用 UTC 时间。
/// 这是一个简化的实现，适用于 2000-2099 年范围。
fn unix_to_ymdhms(mut secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    // 时分秒
    let second = secs % 60;
    secs /= 60;
    let minute = secs % 60;
    secs /= 60;
    let hour = secs % 24;
    secs /= 24;

    // 从 1970-01-01 开始计算年月日
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if secs < days_in_year {
            break;
        }
        secs -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let days_in_month: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];

    let mut month = 1u64;
    for &dim in &days_in_month {
        if secs < dim {
            break;
        }
        secs -= dim;
        month += 1;
    }

    let day = secs + 1; // 日从 1 开始

    (year, month, day, hour, minute, second)
}

fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// 初始化性能记录器，创建输出目录和带时间戳的 CSV 文件并写入表头。
fn init_perf_logger(mut commands: Commands, config: Res<PerfLoggerConfig>) {
    if !config.enabled {
        return;
    }

    // 创建输出目录（如果不存在）
    if let Err(e) = fs::create_dir_all(&config.output_dir) {
        error!("无法创建性能日志目录 {:?}: {}", config.output_dir, e);
        return;
    }

    // 生成带时间戳的文件名
    let filename = generate_timestamp_filename();
    let file_path = config.output_dir.join(&filename);

    let file = match File::create(&file_path) {
        Ok(f) => f,
        Err(e) => {
            error!("无法创建性能日志文件 {:?}: {}", file_path, e);
            return;
        }
    };

    let mut writer = BufWriter::new(file);

    // 写入 CSV 表头
    if let Err(e) = writeln!(
        writer,
        "elapsed_secs,fps,frame_time_ms,chunk_count,triangle_count_gpu,triangle_count_cpu"
    ) {
        error!("写入 CSV 表头失败: {}", e);
        return;
    }

    info!(
        "性能记录器已启动，输出文件: {:?}，间隔: {}s",
        file_path, config.interval_secs
    );

    commands.insert_resource(PerfLoggerState {
        writer,
        timer: Timer::from_seconds(config.interval_secs, TimerMode::Repeating),
        start_time: Instant::now(),
        frame_count: 0,
        diagnostics_printed: false,
        triangle_paths: Vec::new(),
    });
}

/// 自动发现包含三角面信息的诊断路径。
/// 通过遍历 DiagnosticsStore 中所有已注册的诊断，查找路径中包含 "triangle" 的条目。
fn discover_triangle_diagnostics(diagnostics: &DiagnosticsStore) -> Vec<DiagnosticPath> {
    let mut triangle_paths = Vec::new();

    // 遍历所有已注册的诊断（iter() 返回 impl Iterator<Item = &Diagnostic>）
    for diag in diagnostics.iter() {
        let path = diag.path();
        let path_str = path.as_str();
        // 查找路径中包含 "triangle" 的诊断
        if path_str.to_lowercase().contains("triangle") {
            triangle_paths.push(path.clone());
        }
    }

    triangle_paths
}

/// 定期记录性能指标到 CSV 文件。
fn record_perf_metrics(
    time: Res<Time>,
    config: Res<PerfLoggerConfig>,
    diagnostics: Res<DiagnosticsStore>,
    loaded_chunks: Res<LoadedChunks>,
    meshes: Res<Assets<Mesh>>,
    mesh_query: Query<&Mesh3d>,
    mut state: Option<ResMut<PerfLoggerState>>,
) {
    if !config.enabled {
        return;
    }

    let Some(ref mut state) = state else {
        return;
    };

    state.timer.tick(time.delta());
    state.frame_count += 1;

    if !state.timer.just_finished() {
        return;
    }

    let elapsed = state.start_time.elapsed().as_secs_f64();

    // 获取 FPS
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);

    // 获取帧时间（毫秒）
    // Bevy 的 FRAME_TIME 诊断值已经是毫秒单位，无需额外转换
    let frame_time_ms = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);

    // 获取区块数量
    let chunk_count = loaded_chunks.entries.len();

    // 首次运行时自动发现三角面诊断路径
    if !state.diagnostics_printed {
        state.diagnostics_printed = true;

        info!("=== GPU 三角面诊断自动发现 ===");

        // 自动发现三角面诊断路径
        let discovered = discover_triangle_diagnostics(&diagnostics);
        state.triangle_paths = discovered;

        if state.triangle_paths.is_empty() {
            info!("  ❌ 未找到任何三角面诊断路径。");
            info!("  可能原因：");
            info!("    1. GPU 不支持 PIPELINE_STATISTICS_QUERIES 特性");
            info!("    2. RenderDiagnosticsPlugin 未正确注册");
            info!("    3. Bevy 0.18.1 的诊断系统尚未生成三角面数据");
            info!("  当前将使用 CPU 端 Mesh 统计作为回退方案。");

            // 打印所有已注册的诊断路径（用于调试）
            info!("  已注册的诊断路径：");
            for diag in diagnostics.iter() {
                info!("    - {}", diag.path().as_str());
            }
        } else {
            info!(
                "  ✅ 发现 {} 个三角面诊断路径：",
                state.triangle_paths.len()
            );
            for path in &state.triangle_paths {
                if let Some(diag) = diagnostics.get(path) {
                    if let Some(value) = diag.smoothed() {
                        info!("    - {} = {:.0}", path.as_str(), value);
                    } else {
                        info!("    - {} (无数据)", path.as_str());
                    }
                }
            }
        }

        info!("========================");
    }

    // 获取 GPU 三角形数（使用自动发现的路径）
    let mut gpu_triangles: Option<f64> = None;
    for path in &state.triangle_paths {
        if let Some(diag) = diagnostics.get(path) {
            if let Some(value) = diag.smoothed() {
                *gpu_triangles.get_or_insert(0.0) += value;
            }
        }
    }

    // 回退：统计 Mesh 数据中的三角形数
    let cpu_triangles: u32 = mesh_query
        .iter()
        .map(|h| {
            meshes.get(&h.0).map_or(0, |mesh| match mesh.indices() {
                Some(indices) => indices.len() as u32 / 3,
                None => mesh.count_vertices() as u32 / 3,
            })
        })
        .sum();

    // 写入 CSV 行
    let gpu_tri_str = gpu_triangles
        .map(|v| format!("{:.0}", v))
        .unwrap_or_else(|| "N/A".to_string());

    if let Err(e) = writeln!(
        state.writer,
        "{:.2},{:.1},{:.3},{},{},{}",
        elapsed, fps, frame_time_ms, chunk_count, gpu_tri_str, cpu_triangles
    ) {
        error!("写入性能日志失败: {}", e);
    }

    // 定期 flush 确保数据写入磁盘
    if let Err(e) = state.writer.flush() {
        error!("刷新性能日志文件失败: {}", e);
    }
}
