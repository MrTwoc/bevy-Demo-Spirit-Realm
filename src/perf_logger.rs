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
use crate::hud::CachedTriangleCount;

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
    writer: Option<BufWriter<File>>,  // 改为 Option，禁用时为 None
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
            .add_systems(
                Update,
                (
                    toggle_perf_logger,
                    manage_perf_file,
                    record_perf_metrics.run_if(|config: Res<PerfLoggerConfig>| config.enabled),
                ),
            );
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

/// 初始化性能记录器。
/// 总是创建 PerfLoggerState 资源，但仅在 enabled=true 时才打开日志文件。
fn init_perf_logger(mut commands: Commands, config: Res<PerfLoggerConfig>) {
    // 创建输出目录（如果不存在且已启用）
    if config.enabled {
        if let Err(e) = fs::create_dir_all(&config.output_dir) {
            error!("无法创建性能日志目录 {:?}: {}", config.output_dir, e);
        }
    }

    // 如果启用，打开日志文件并写入表头
    let writer = if config.enabled {
        let filename = generate_timestamp_filename();
        let file_path = config.output_dir.join(&filename);

        match File::create(&file_path) {
            Ok(file) => {
                let mut writer = BufWriter::new(file);

                // 写入 CSV 表头
                if let Err(e) = writeln!(
                    writer,
                    "elapsed_secs,fps,frame_time_ms,chunk_count,triangle_count_cpu"
                ) {
                    error!("写入 CSV 表头失败: {}", e);
                    None
                } else {
                    info!(
                        "性能记录器已启动，输出文件: {:?}，间隔: {}s",
                        file_path, config.interval_secs
                    );
                    Some(writer)
                }
            }
            Err(e) => {
                error!("无法创建性能日志文件 {:?}: {}", file_path, e);
                None
            }
        }
    } else {
        None
    };

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

/// 监听 F2 按键，切换性能记录器的启用状态。
fn toggle_perf_logger(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut config: ResMut<PerfLoggerConfig>,
) {
    if keyboard_input.just_pressed(KeyCode::F2) {
        config.enabled = !config.enabled;
        if config.enabled {
            info!("性能记录器已启用 (F2 禁用)");
        } else {
            info!("性能记录器已禁用 (F2 启用)");
        }
    }
}

/// 管理性能日志文件的打开和关闭。
/// 当 enabled 从 false 变为 true 时，打开新的日志文件；
/// 当 enabled 从 true 变为 false 时，关闭当前文件并 flush。
fn manage_perf_file(
    config: Res<PerfLoggerConfig>,
    mut state: Option<ResMut<PerfLoggerState>>,
    mut commands: Commands,
) {
    // 如果 PerfLoggerState 不存在，创建它（防御性编程）
    let state = match state {
        Some(s) => s,
        None => {
            // 这不应该发生，因为 init_perf_logger 总是创建它
            commands.insert_resource(PerfLoggerState {
                writer: None,
                timer: Timer::from_seconds(config.interval_secs, TimerMode::Repeating),
                start_time: Instant::now(),
                frame_count: 0,
                diagnostics_printed: false,
                triangle_paths: Vec::new(),
            });
            return;
        }
    };

    let state = state.into_inner();

    if config.enabled && state.writer.is_none() {
        // 需要打开新文件
        if let Err(e) = fs::create_dir_all(&config.output_dir) {
            error!("无法创建性能日志目录 {:?}: {}", config.output_dir, e);
            return;
        }

        let filename = generate_timestamp_filename();
        let file_path = config.output_dir.join(&filename);

        match File::create(&file_path) {
            Ok(file) => {
                let mut writer = BufWriter::new(file);

                // 写入 CSV 表头
                if let Err(e) = writeln!(
                    writer,
                    "elapsed_secs,fps,frame_time_ms,chunk_count,triangle_count_cpu"
                ) {
                    error!("写入 CSV 表头失败: {}", e);
                } else {
                    info!(
                        "性能记录器已启动，输出文件: {:?}，间隔: {}s",
                        file_path, config.interval_secs
                    );
                    state.writer = Some(writer);
                    state.start_time = Instant::now();  // 重置开始时间
                    state.frame_count = 0;  // 重置帧计数
                }
            }
            Err(e) => {
                error!("无法创建性能日志文件 {:?}: {}", file_path, e);
            }
        }
    } else if !config.enabled && state.writer.is_some() {
        // 需要关闭文件
        if let Some(mut writer) = state.writer.take() {
            if let Err(e) = writer.flush() {
                error!("刷新性能日志文件失败: {}", e);
            }
            info!("性能记录器已禁用，日志文件已关闭");
        }
    }
}

/// 定期记录性能指标到 CSV 文件。
fn record_perf_metrics(
    time: Res<Time>,
    diagnostics: Res<DiagnosticsStore>,
    loaded_chunks: Res<LoadedChunks>,
    cached_triangles: Res<CachedTriangleCount>,
    mut state: Option<ResMut<PerfLoggerState>>,
) {
    let Some(mut state) = state else {
        return;
    };

    // 自动发现三角面诊断路径（仅首次运行时）
    if !state.diagnostics_printed {
        state.triangle_paths = discover_triangle_diagnostics(&diagnostics);
        if !state.triangle_paths.is_empty() {
            info!("发现三角面诊断路径: {:?}", state.triangle_paths);
        }
        state.diagnostics_printed = true;
    }

    // 先更新计时器和帧计数（不借用 writer）
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

    // 获取 GPU 三角形数（使用自动发现的路径）
    let mut gpu_triangles: Option<f64> = None;
    for path in &state.triangle_paths {
        if let Some(diag) = diagnostics.get(path) {
            if let Some(value) = diag.smoothed() {
                *gpu_triangles.get_or_insert(0.0) += value;
            }
        }
    }

    // 使用增量维护的 CachedTriangleCount（chunk_manager 实时更新），
    // 避免遍历所有 2000+ Mesh3d 实体统计三角形数
    let cpu_triangles = cached_triangles.0;

    // 现在借用 writer 并写入数据
    if let Some(writer) = &mut state.writer {
        // 写入 CSV 行
        if let Err(e) = writeln!(
            writer,
            "{:.2},{:.1},{:.3},{},{}",
            elapsed, fps, frame_time_ms, chunk_count, cpu_triangles
        ) {
            error!("写入性能日志失败: {}", e);
        }

        // 定期 flush 确保数据写入磁盘
        if let Err(e) = writer.flush() {
            error!("刷新性能日志文件失败: {}", e);
        }
    }
}
