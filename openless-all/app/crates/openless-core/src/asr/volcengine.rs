#![cfg_attr(target_os = "linux", allow(dead_code, unused_variables))]
//! Volcengine SAUC bigmodel streaming ASR client.
//!
//! Sends PCM frames, combines recognition updates and waits for the protocol's
//! final response. A `definite=true` utterance commits that segment only; it does
//! not end the stream or prevent later audio from being sent.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex as ParkingMutex;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex, Notify};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::HeaderValue;
use tokio_tungstenite::tungstenite::{handshake::client::Request as WebSocketRequest, Message};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use crate::config::{TaskSpawner, TokioTaskSpawner};

use super::frame::{self, Flags, MessageType, Serialization};
use super::{AudioConsumer, DictionaryHotword, RawTranscript};
use crate::ports::{TextStreamChunk, TextStreamSink};

/// 官方「大模型流式语音识别 API」（双向流式·优化版）端点：
/// https://www.volcengine.com/docs/6561/1354869
/// 新旧两种鉴权模式共享同一端点，仅握手鉴权头不同。
const ENDPOINT_APP_ID_TOKEN: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";
const ENDPOINT_API_KEY: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";
/// Agent Plan uses a dedicated subscription endpoint with API-key authentication.
/// https://docs.volcengine.com/docs/82379/2516286
const ENDPOINT_AGENT_PLAN: &str = "wss://openspeech.bytedance.com/api/v3/plan/sauc/bigmodel_async";
/// 200 ms of 16 kHz / 16-bit / mono PCM.
pub const TARGET_AUDIO_CHUNK_BYTES: usize = 6_400;
/// 16 kHz · 16-bit · mono = 32 000 bytes/sec → 32 bytes/ms.
const BYTES_PER_MS: f64 = 32.0;
const HOTWORD_CAP: usize = 80;
/// dialog_ctx（语境提示）的注入上限：官方限 800 tokens（docs/6561/1354869），
/// 中文按 1 字符 ≈ 1 token 保守估算，总字符数截到该值内（超出丢最旧，保住
/// 最近的话）。来源：会话启动时冻结的最近语音背景（批次 A）＋固定场景条目。
const DIALOG_CTX_CHAR_CAP: usize = 800;
/// dialog_ctx 的固定场景条目（2026-09-18 用户裁决；官方 dialog_ctx 示例 d 类
/// 「业务/个性化信息」）：告诉识别引擎当前是「语音给 AI 助手下指令」的场景，
/// 内容多为待办与任务类表达。代码固定常量（逐字钉住，见测试），常驻计入
/// 800 字符预算——它不是用户数据，不受最近语音背景开关控制（否则该开关
/// 默认关时场景偏置就永不生效）。
pub const DIALOG_CTX_SCENE_ENTRY: &str = "用户在通过语音给 AI 助手下指令，内容为待办与任务类表达";
/// 决策 3 输入框偏置（2026-09-18 用户裁决）条目前缀：对话会话启动时读取
/// 光标所在输入框（其它应用）的已有文本，作为 dialog_ctx 的第三路语境
/// （豆包同款机制），条目形态 `输入框已有内容：{截断后的文本}`。
pub const DIALOG_CTX_INPUT_BOX_PREFIX: &str = "输入框已有内容：";
/// 输入框文本的单条字符上限（超长取尾部——光标附近的最新内容对识别最相关）。
/// 在 dialog_ctx 800 字符预算内自行分配：场景＋输入框 ≤ 436 字符，给最近语音
/// 留出余量。
pub const INPUT_BOX_ENTRY_CHAR_CAP: usize = 400;
const FINAL_RESULT_TIMEOUT: Duration = Duration::from_secs(12);

/// 弱网下 TLS/WebSocket 握手可能一直挂到 OS 级 TCP 超时（几十秒），期间用户卡在
/// 「Starting」无法语音输入。协调器的全局超时只覆盖 `await_final_result`，**不**覆盖
/// `open_session`，所以这里必须自己给握手设上限：超时即快速失败并重试，而不是冻结。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 单次网络抖动（连接被重置 / 瞬时 DNS 失败）以前会直接让整次听写失败。重试几次让
/// 抖动可恢复。`AuthRejected`（凭据被拒）不在重试之列——重试也不会变好，只会拖慢报错。
const CONNECT_MAX_ATTEMPTS: usize = 3;
const CONNECT_RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// Volcengine ASR 鉴权模式。
///
/// - `AppIdToken`：旧版语音控制台应用，使用 `X-Api-App-Key` + `X-Api-Access-Key` 双表头鉴权。
/// - `ApiKey`：普通服务 API Key 或 Agent Plan 专属 API Key，使用单个 `X-Api-Key` 表头鉴权。
///
/// 普通服务下，两种模式共享 WebSocket 端点与二进制帧协议，仅握手鉴权头不同。
/// Agent Plan 按服务选择专属端点，并固定使用 ApiKey 鉴权。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VolcengineAuthMode {
    AppIdToken,
    ApiKey,
}

impl VolcengineAuthMode {
    pub fn parse(s: &str) -> Self {
        match s {
            "api_key" => Self::ApiKey,
            _ => Self::AppIdToken,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AppIdToken => "app_id_token",
            Self::ApiKey => "api_key",
        }
    }

    /// 当前模式下所需凭据是否齐备（统一 trim 语义）。
    ///
    /// `secret` 的语义随模式：AppIdToken = Access Token（旧版语音控制台），
    /// ApiKey = 普通服务或 Agent Plan 的 ASR API Key。`app_id` 仅在 AppIdToken 模式要求非空。
    ///
    /// 所有按模式判定凭据完整性的入口（`open_session`、`volcengine_configured`、
    /// `ensure_asr_credentials`）都应复用此方法，避免三处规则漂移。
    pub fn auth_ok(&self, app_id: &str, secret: &str) -> bool {
        let app_id_ok = match self {
            Self::AppIdToken => !app_id.trim().is_empty(),
            Self::ApiKey => true,
        };
        app_id_ok && !secret.trim().is_empty()
    }
}

/// Service selection is separate from the standard service's authentication mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VolcengineService {
    #[default]
    Standard,
    AgentPlan,
}

impl VolcengineService {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value.trim() {
            "" | "standard" => Ok(Self::Standard),
            "agent_plan" => Ok(Self::AgentPlan),
            _ => Err("volcengineServiceInvalid"),
        }
    }

    pub fn auth_mode(self, configured: VolcengineAuthMode) -> VolcengineAuthMode {
        match self {
            Self::Standard => configured,
            Self::AgentPlan => VolcengineAuthMode::ApiKey,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VolcengineCredentials {
    pub service: VolcengineService,
    pub auth_mode: VolcengineAuthMode,
    /// App ID（AppIdToken 模式使用；ApiKey 模式下为空）。
    pub app_id: String,
    /// Access Token（AppIdToken 模式下）或 API Key（ApiKey 模式下）。
    pub access_token: String,
    pub resource_id: String,
}

impl VolcengineCredentials {
    pub fn default_resource_id() -> &'static str {
        "volc.seedasr.sauc.duration"
    }

    /// 未配置或仅含空白字符时使用默认 Resource ID；保留非空配置的原始值。
    pub fn resolve_resource_id(configured: Option<String>) -> String {
        configured
            .filter(|resource_id| !resource_id.trim().is_empty())
            .unwrap_or_else(|| Self::default_resource_id().to_string())
    }

    /// 凭据是否满足当前鉴权模式的要求（统一 trim 语义，见 [`VolcengineAuthMode::auth_ok`]）。
    pub fn auth_ok(&self) -> bool {
        self.service
            .auth_mode(self.auth_mode.clone())
            .auth_ok(&self.app_id, &self.access_token)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VolcengineASRError {
    #[error("credentials missing")]
    CredentialsMissing,
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    /// WebSocket 握手阶段服务端返回 401 / 403：凭据被拒。
    /// 区分自 `ConnectionFailed`（DNS/TLS/网络层失败）—— 前者通常是 App ID / Access
    /// Token / Resource ID 错或账号没开通 bigmodel；后者是网络断 / 防火墙 / DNS。
    /// 文案简短，原因在文档里说明，capsule 不堆长引导。
    #[error("凭据被拒（{0}）")]
    AuthRejected(u16),
    /// WebSocket 握手阶段服务端返回 429：请求过多 / 账号被限流。
    /// 单独归类而非落入 `ConnectionFailed` —— 后者会被 `connect_with_retry` 当网络抖动
    /// 立即重试 3 次，反而加剧限流、且文案含糊指向「网络失败」误导用户。此类与
    /// `AuthRejected` 一样**短路不重试**：立即回带明确文案，让用户知道是限流不是断网。
    #[error("请求过多，账号被限流（{0}）")]
    RateLimited(u16),
    #[error("no final result")]
    NoFinalResult,
    #[error("final result timed out")]
    FinalResultTimeout,
    #[error("decode failed: {0}")]
    DecodeFailed(String),
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
type SharedWriter = Arc<AsyncMutex<Option<WsSink>>>;
type AudioFrameSender = mpsc::UnboundedSender<(i32, Vec<u8>)>;

/// Sync state shared across the receive loop, the public API, and the
/// audio-consumer fast path.
#[derive(Default)]
struct SyncState {
    pending_audio: Vec<u8>,
    next_sequence: i32,
    bytes_sent: usize,
    frames_sent: usize,
    is_connected: bool,
    final_tx: Option<oneshot::Sender<Result<RawTranscript, VolcengineASRError>>>,
    start: Option<Instant>,
    /// 最近一次 partial（非 final）的累积 transcript。服务端在 final 帧到达前
    /// 关闭连接 / 网络中断时，作为 fallback 回给上层，避免「用户的话已经识别出来
    /// 但没拿到 final」就丢光。
    last_partial_text: String,
}

pub struct VolcengineStreamingASR {
    credentials: VolcengineCredentials,
    task_spawner: Arc<dyn TaskSpawner>,
    hotwords: Vec<DictionaryHotword>,
    /// 会话启动时冻结的最近语音背景（新→旧）：dialog_ctx 语境提示的注入内容。
    /// 空＝无历史，不注入（零变化）。
    dialog_ctx: Vec<String>,
    /// 二遍复核（2026-09-18 用户裁决）：官方 `enable_nonstream` 参数——判停的
    /// 分句用非流式模型重识别，更准稍慢。偏好 `asr_second_pass_enabled` 冻结，
    /// false 时不携带该键。
    second_pass_enabled: bool,
    state: ParkingMutex<SyncState>,
    /// Guards the WebSocket write half so concurrent `send` calls serialize.
    /// Stored as Arc so spawned send tasks can hold their own clone — independent
    /// of the lifetime of any particular `&self` borrow.
    writer: SharedWriter,
    final_rx: ParkingMutex<Option<oneshot::Receiver<Result<RawTranscript, VolcengineASRError>>>>,
    /// 单 worker 模式：consume_pcm_chunk 把 (seq, chunk) 入队这个 channel，
    /// open_session 里 spawn 出的唯一 worker 串行 recv + send_binary，
    /// 保证 seq 顺序严格等于实际发送顺序。session 结束时 take() 掉这个 sender，
    /// worker 的 recv() 返回 None 自动退出。
    audio_tx: ParkingMutex<Option<AudioFrameSender>>,
    /// 队列里 + worker 在飞的 audio 帧总数。consume +N，worker send 完一帧 -1。
    /// send_last_frame 必须等它降到 0 才能安全发末帧，否则末帧可能被服务端先收到
    /// 而把后续 chunk 当成「stream 已结束」之后的多余数据丢弃 → 尾句丢失。
    pending_sends: Arc<AtomicUsize>,
    send_done: Arc<Notify>,
    partial_sink: ParkingMutex<Option<Arc<dyn TextStreamSink>>>,
}

impl VolcengineStreamingASR {
    pub fn new(
        credentials: VolcengineCredentials,
        hotwords: Vec<DictionaryHotword>,
        dialog_ctx: Vec<String>,
        second_pass_enabled: bool,
    ) -> Self {
        Self::with_task_spawner(
            credentials,
            hotwords,
            dialog_ctx,
            second_pass_enabled,
            Arc::new(TokioTaskSpawner),
        )
    }

    pub fn with_task_spawner(
        credentials: VolcengineCredentials,
        hotwords: Vec<DictionaryHotword>,
        dialog_ctx: Vec<String>,
        second_pass_enabled: bool,
        task_spawner: Arc<dyn TaskSpawner>,
    ) -> Self {
        Self {
            credentials,
            task_spawner,
            hotwords,
            dialog_ctx,
            second_pass_enabled,
            state: ParkingMutex::new(SyncState::default()),
            writer: Arc::new(AsyncMutex::new(None)),
            final_rx: ParkingMutex::new(None),
            audio_tx: ParkingMutex::new(None),
            pending_sends: Arc::new(AtomicUsize::new(0)),
            send_done: Arc::new(Notify::new()),
            partial_sink: ParkingMutex::new(None),
        }
    }

    pub fn set_partial_sink(&self, sink: Arc<dyn TextStreamSink>) {
        *self.partial_sink.lock() = Some(sink);
    }

    pub async fn open_session(self: &Arc<Self>) -> Result<(), VolcengineASRError> {
        let creds = &self.credentials;
        // 统一走 VolcengineCredentials::auth_ok（trim 语义），与概览页凭据状态检测、
        // dictation 预检保持同一判定规则。
        if !creds.auth_ok() || creds.resource_id.trim().is_empty() {
            return Err(VolcengineASRError::CredentialsMissing);
        }

        let connect_id = Uuid::new_v4().to_string();
        let ws = self.connect_with_retry(&connect_id).await?;
        let (write, read) = ws.split();

        let (tx, rx) = oneshot::channel();

        // Reset sync state for the new session.
        {
            let mut st = self.state.lock();
            st.pending_audio.clear();
            st.next_sequence = 1;
            st.bytes_sent = 0;
            st.frames_sent = 0;
            st.is_connected = true;
            st.final_tx = Some(tx);
            st.start = Some(Instant::now());
            st.last_partial_text.clear();
        }
        self.pending_sends.store(0, Ordering::SeqCst);
        *self.final_rx.lock() = Some(rx);
        *self.writer.lock().await = Some(write);

        // 起一个唯一的 audio worker：consume_pcm_chunk 把 (seq, chunk) 推到 audio_tx，
        // worker 这边 FIFO recv 然后串行 send_binary。session 结束后调用方
        // (cancel / handle_frame error / fallback_to_partial_or_error) 会 take 掉
        // self.audio_tx，channel 关闭，worker 自然退出。
        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<(i32, Vec<u8>)>();
        *self.audio_tx.lock() = Some(audio_tx);
        let writer_for_worker = Arc::clone(&self.writer);
        let pending_for_worker = Arc::clone(&self.pending_sends);
        let notify_for_worker = Arc::clone(&self.send_done);
        let task_spawner = Arc::clone(&self.task_spawner);
        task_spawner.spawn(Box::pin(async move {
            while let Some((seq, chunk)) = audio_rx.recv().await {
                let frame = frame::build(
                    MessageType::AudioOnlyRequest,
                    Flags::PositiveSequence,
                    Serialization::None,
                    &chunk,
                    Some(seq),
                );
                if let Err(e) = send_binary(&writer_for_worker, frame).await {
                    log::error!("[asr] audio frame seq={} send 失败: {}", seq, e);
                }
                if pending_for_worker.fetch_sub(1, Ordering::SeqCst) == 1 {
                    notify_for_worker.notify_waiters();
                }
            }
        }));

        // Send the first frame: full client request with seq=1.
        let payload_json = self.build_first_frame_payload(&connect_id);
        let payload_bytes = serde_json::to_vec(&payload_json)
            .map_err(|e| VolcengineASRError::DecodeFailed(e.to_string()))?;
        let first_seq = self.allocate_positive_seq();
        let frame = frame::build(
            MessageType::FullClientRequest,
            Flags::PositiveSequence,
            Serialization::Json,
            &payload_bytes,
            Some(first_seq),
        );
        send_binary(&self.writer, frame).await?;

        // Spawn the receive loop. Holds a Weak<Self> so it doesn't keep
        // the struct alive forever if callers drop their Arcs.
        let weak_self = Arc::downgrade(self);
        let task_spawner = Arc::clone(&self.task_spawner);
        task_spawner.spawn(Box::pin(async move {
            let mut read = read;
            while let Some(msg) = read.next().await {
                let Some(this) = weak_self.upgrade() else {
                    break;
                };
                match msg {
                    Ok(Message::Binary(data)) => {
                        if !this.handle_frame(&data) {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) => {
                        // 服务端没发 final 就关连接 → 用最近一次 partial 兜底，不丢已识别的文字。
                        this.fallback_to_partial_or_error(VolcengineASRError::NoFinalResult);
                        break;
                    }
                    Ok(_) => { /* ignore text/ping/pong */ }
                    Err(e) => {
                        log::error!("[asr] receive loop error: {}", e);
                        // 网络中断同样回退到 partial，让用户至少拿到已经识别的部分。
                        this.fallback_to_partial_or_error(VolcengineASRError::ConnectionFailed(
                            e.to_string(),
                        ));
                        break;
                    }
                }
                if !this.state.lock().is_connected {
                    break;
                }
            }
        }));

        Ok(())
    }

    /// Build the WebSocket handshake request (endpoint + auth headers). Rebuilt
    /// per connect attempt because `connect_async` consumes the request and
    /// `http::Request` is not `Clone`.
    fn build_connect_request(
        &self,
        connect_id: &str,
        request_id: &str,
    ) -> Result<WebSocketRequest, VolcengineASRError> {
        let auth_mode = self
            .credentials
            .service
            .auth_mode(self.credentials.auth_mode.clone());
        let endpoint = match (self.credentials.service, &auth_mode) {
            (VolcengineService::AgentPlan, _) => ENDPOINT_AGENT_PLAN,
            (_, VolcengineAuthMode::AppIdToken) => ENDPOINT_APP_ID_TOKEN,
            (_, VolcengineAuthMode::ApiKey) => ENDPOINT_API_KEY,
        };
        let mut request = endpoint
            .into_client_request()
            .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?;
        let headers = request.headers_mut();

        // 根据鉴权模式选择表头：
        // - AppIdToken：X-Api-App-Key + X-Api-Access-Key（旧版语音控制台）
        // - ApiKey：X-Api-Key（普通服务或 Agent Plan 的 ASR API Key，单头即可）
        match auth_mode {
            VolcengineAuthMode::AppIdToken => {
                headers.insert(
                    "X-Api-App-Key",
                    HeaderValue::from_str(&self.credentials.app_id)
                        .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?,
                );
                headers.insert(
                    "X-Api-Access-Key",
                    HeaderValue::from_str(&self.credentials.access_token)
                        .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?,
                );
            }
            VolcengineAuthMode::ApiKey => {
                headers.insert(
                    "X-Api-Key",
                    HeaderValue::from_str(&self.credentials.access_token)
                        .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?,
                );
            }
        }

        headers.insert(
            "X-Api-Resource-Id",
            HeaderValue::from_str(&self.credentials.resource_id)
                .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?,
        );
        headers.insert(
            "X-Api-Connect-Id",
            HeaderValue::from_str(connect_id)
                .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?,
        );
        // 官方鉴权表（docs/6561/1354869）要求其余两个头：
        // X-Api-Request-Id（任务 ID，官方推荐随机 UUID；每次握手尝试独立生成）与
        // X-Api-Sequence（发包序号，固定值 -1）。
        headers.insert(
            "X-Api-Request-Id",
            HeaderValue::from_str(request_id)
                .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))?,
        );
        headers.insert("X-Api-Sequence", HeaderValue::from_static("-1"));
        Ok(request)
    }

    /// Connect with a per-attempt timeout and bounded retries so a poor network
    /// (hung handshake or a transient blip) doesn't kill the whole dictation.
    /// `AuthRejected` / `RateLimited` short-circuit — bad credentials never heal on
    /// retry, and hammering a rate-limited account only makes the throttle worse.
    async fn connect_with_retry(&self, connect_id: &str) -> Result<WsStream, VolcengineASRError> {
        let mut attempt = 0usize;
        loop {
            attempt += 1;
            let request_id = Uuid::new_v4().to_string();
            let request = self.build_connect_request(connect_id, &request_id)?;
            log::info!(
                "[asr] Volcengine connect endpoint={} connect_id={} request_id={}",
                request.uri(),
                connect_id,
                request_id
            );
            match tokio::time::timeout(CONNECT_TIMEOUT, connect_async(request)).await {
                Ok(Ok((ws, response))) => {
                    log::info!(
                        "[asr] Volcengine connected connect_id={} log_id={}",
                        connect_id,
                        response
                            .headers()
                            .get("X-Tt-Logid")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or("-")
                    );
                    return Ok(ws);
                }
                Ok(Err(e)) => {
                    let classified = classify_connect_error(e);
                    if is_non_retryable(&classified) || attempt >= CONNECT_MAX_ATTEMPTS {
                        return Err(classified);
                    }
                    log::warn!(
                        "[asr] 连接尝试 {attempt}/{CONNECT_MAX_ATTEMPTS} 失败: {classified}；重试中"
                    );
                }
                Err(_) => {
                    if attempt >= CONNECT_MAX_ATTEMPTS {
                        return Err(VolcengineASRError::ConnectionFailed(format!(
                            "连接超时（{} ms）",
                            CONNECT_TIMEOUT.as_millis()
                        )));
                    }
                    log::warn!(
                        "[asr] 连接尝试 {attempt}/{CONNECT_MAX_ATTEMPTS} 超时（{} ms）；重试中",
                        CONNECT_TIMEOUT.as_millis()
                    );
                }
            }
            tokio::time::sleep(CONNECT_RETRY_BACKOFF * attempt as u32).await;
        }
    }

    pub async fn send_last_frame(&self) -> Result<(), VolcengineASRError> {
        // 等所有 fire-and-forget 发送完成。否则末帧（NegativeSequence）可能比尾部
        // chunk 先到服务端，被识别为「流已结束」之后再到的 chunk 全部丢弃 = 尾句吞掉。
        // 给一个 800ms 上限避免极端网络下永远等。
        let drain_deadline = Instant::now() + std::time::Duration::from_millis(800);
        while self.pending_sends.load(Ordering::SeqCst) > 0 {
            let remaining = drain_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                log::warn!(
                    "[asr] send_last_frame: pending {} 帧未发送完，超时强制继续",
                    self.pending_sends.load(Ordering::SeqCst)
                );
                break;
            }
            // notified() 返回 future，被 timeout 包住 → 等待发送完成或超时
            let _ = tokio::time::timeout(remaining, self.send_done.notified()).await;
        }

        // Drain leftover audio (if any) into one final positive-sequence frame.
        let leftover = {
            let mut st = self.state.lock();
            if st.pending_audio.is_empty() {
                None
            } else {
                Some(std::mem::take(&mut st.pending_audio))
            }
        };

        if let Some(buf) = leftover {
            let seq = self.allocate_positive_seq();
            let len = buf.len();
            let frame = frame::build(
                MessageType::AudioOnlyRequest,
                Flags::PositiveSequence,
                Serialization::None,
                &buf,
                Some(seq),
            );
            {
                let mut st = self.state.lock();
                st.bytes_sent += len;
                st.frames_sent += 1;
            }
            send_binary(&self.writer, frame).await?;
        }

        // Final frame: negativeSequence + negative seq number signals stream end.
        // 末帧用 negativeSequence + 负序号收尾，告诉服务端"流到此结束"。
        let final_seq = {
            let mut st = self.state.lock();
            let s = -st.next_sequence;
            st.next_sequence += 1;
            s
        };
        let frame = frame::build(
            MessageType::AudioOnlyRequest,
            Flags::NegativeSequence,
            Serialization::None,
            &[],
            Some(final_seq),
        );
        send_binary(&self.writer, frame).await?;

        let (total_bytes, total_frames) = {
            let st = self.state.lock();
            (st.bytes_sent, st.frames_sent)
        };
        let duration_ms = (total_bytes as f64 / BYTES_PER_MS) as u64;
        log::info!(
            "[asr] 发送总结：{} audio frames, {} bytes (~{} ms)",
            total_frames,
            total_bytes,
            duration_ms
        );
        Ok(())
    }

    pub async fn await_final_result(&self) -> Result<RawTranscript, VolcengineASRError> {
        self.await_final_result_with_timeout(FINAL_RESULT_TIMEOUT)
            .await
    }

    pub async fn await_final_result_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<RawTranscript, VolcengineASRError> {
        let rx = self.final_rx.lock().take();
        let Some(rx) = rx else {
            return Err(VolcengineASRError::NoFinalResult);
        };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(VolcengineASRError::NoFinalResult),
            Err(_) => {
                log::error!(
                    "[asr] final result timed out after {} ms",
                    timeout.as_millis()
                );
                self.cancel();
                Err(VolcengineASRError::FinalResultTimeout)
            }
        }
    }

    pub fn cancel(&self) {
        {
            let mut st = self.state.lock();
            st.is_connected = false;
            st.pending_audio.clear();
        }
        // Drop audio sender → worker.recv() 返回 None → worker 退出，不再 hold writer。
        *self.audio_tx.lock() = None;
        // Close the writer asynchronously so the receive loop sees EOF. The
        // host-provided spawner also handles synchronous teardown callers.
        let writer = Arc::clone(&self.writer);
        self.task_spawner.spawn(Box::pin(async move {
            if let Some(mut w) = writer.lock().await.take() {
                let _ = w.close().await;
            }
        }));
        self.signal_error(VolcengineASRError::NoFinalResult);
    }

    // ---- internals ----

    fn build_first_frame_payload(&self, connect_id: &str) -> Value {
        let mut request = json!({
            "model_name": "bigmodel",
            "enable_itn": true,
            "enable_punc": true,
            "show_utterances": true,
            "enable_speaker_info": true,
        });
        // 二遍复核：判停分句用非流式模型重识别（docs/6561/1354869）。关闭时
        // 不携带该键——旧行为零歧义，服务端按默认关处理。
        if self.second_pass_enabled {
            request["enable_nonstream"] = Value::Bool(true);
        }
        if let Some(context) = context_payload(&self.hotwords, &self.dialog_ctx) {
            request["context"] = Value::String(context);
            let enabled_count = self.hotwords.iter().filter(|h| h.enabled).count();
            log::info!(
                "[asr] hotwords injected: {}; dialog_ctx entries: {}",
                enabled_count,
                self.dialog_ctx.len()
            );
        }
        json!({
            "user": { "uid": connect_id },
            "audio": {
                "format": "pcm",
                "rate": 16000,
                "bits": 16,
                "channel": 1,
                "codec": "raw",
            },
            "request": request,
        })
    }

    fn allocate_positive_seq(&self) -> i32 {
        let mut st = self.state.lock();
        let s = st.next_sequence;
        st.next_sequence += 1;
        s
    }

    /// Returns `false` once the session has terminated (caller should stop reading).
    fn handle_frame(&self, data: &[u8]) -> bool {
        let Some(parsed) = frame::parse(data) else {
            log::error!("[asr] 帧解析失败 raw={}", hex_prefix(data, 32));
            return true;
        };

        if parsed.message_type == Some(MessageType::ErrorMessage) {
            let body = String::from_utf8_lossy(&parsed.payload).to_string();
            let code = parsed.error_code.unwrap_or(0);
            log::error!(
                "[asr] error frame code={} body={}",
                code,
                body.chars().take(200).collect::<String>()
            );
            self.signal_error(VolcengineASRError::ConnectionFailed(format!(
                "ASR error {}: {}",
                code, body
            )));
            self.state.lock().is_connected = false;
            *self.audio_tx.lock() = None;
            return false;
        }

        if parsed.message_type != Some(MessageType::FullServerResponse) {
            return true;
        }

        if let Ok(payload_str) = std::str::from_utf8(&parsed.payload) {
            log::info!(
                "[asr] server JSON: {}",
                payload_str.chars().take(400).collect::<String>()
            );
        }

        let json: Value = match serde_json::from_slice(&parsed.payload) {
            Ok(v) => v,
            Err(_) => return true,
        };
        let Some(result) = normalized_result(&json) else {
            return true;
        };

        // 流结束信号只信帧头 flags（lastPacket / negativeSequence）。
        // 之前误把 utterance.definite=true 当成流结束——但那只代表"这一段语音已固化"，
        // 用户可能还在继续说。结果一收到第一个 definite=true 就关掉接收，
        // 后面用户讲的内容全部丢失（实测丢了 9 秒）。
        let has_final = parsed.is_final();
        let mut full_text = result
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(utterances) = result.get("utterances").and_then(|v| v.as_array()) {
            // --- 声纹过滤：只保留主要说话人（说话时长最长的） ---
            // 1. 统计每个 speaker 的说话时长
            let mut speaker_durations: std::collections::HashMap<String, u64> =
                std::collections::HashMap::new();
            for u in utterances.iter() {
                if let (Some(speaker), Some(start), Some(end)) = (
                    u.get("speaker").and_then(|s| s.as_str()),
                    u.get("start_time").and_then(|t| t.as_u64()),
                    u.get("end_time").and_then(|t| t.as_u64()),
                ) {
                    let dur = end.saturating_sub(start);
                    *speaker_durations.entry(speaker.to_string()).or_insert(0) += dur;
                }
            }

            // 2. 找到说话时长最长的 speaker（即"主要说话人"）
            let primary_speaker: Option<String> = speaker_durations
                .iter()
                .max_by_key(|(_, &dur)| dur)
                .map(|(s, _)| s.clone());

            // 3. 只拼接主要说话人的文本；如果没有任何 speaker 标签，回退到全量拼接
            let pieces: Vec<&str> = if let Some(ref primary) = primary_speaker {
                utterances
                    .iter()
                    .filter(|u| {
                        u.get("speaker")
                            .and_then(|s| s.as_str())
                            .map(|s| s == primary.as_str())
                            .unwrap_or(true) // 无 speaker 字段的 utterance 保留
                    })
                    .filter_map(|u| u.get("text").and_then(|t| t.as_str()))
                    .collect()
            } else {
                utterances
                    .iter()
                    .filter_map(|u| u.get("text").and_then(|t| t.as_str()))
                    .collect()
            };

            if !pieces.is_empty() {
                full_text = pieces.join("");
                if let Some(ref primary) = primary_speaker {
                    let filtered_count = utterances.len().saturating_sub(pieces.len());
                    if filtered_count > 0 {
                        log::info!(
                            "[asr] speaker filter: primary={}, kept={}, filtered={}",
                            primary,
                            pieces.len(),
                            filtered_count
                        );
                    }
                }
            }
        }

        // 缓存最新的 partial transcript：服务端在 final 帧前断连时 fallback 用。
        // 仅在非空且不是 final 时更新（final 走另一条路径）。
        if !has_final && !full_text.is_empty() {
            let delta = {
                let mut state = self.state.lock();
                let delta = full_text
                    .strip_prefix(&state.last_partial_text)
                    .unwrap_or("")
                    .to_string();
                state.last_partial_text = full_text.clone();
                delta
            };
            if !delta.is_empty() {
                if let Some(sink) = self.partial_sink.lock().clone() {
                    let _ = sink.publish(TextStreamChunk {
                        text: delta,
                        offset: 0,
                    });
                }
            }
        }

        if has_final {
            let duration_ms = self
                .state
                .lock()
                .start
                .map(|s| s.elapsed().as_millis() as u64)
                .unwrap_or(0);
            let transcript = RawTranscript {
                text: full_text,
                duration_ms,
            };
            self.signal_success(transcript);
            self.state.lock().is_connected = false;
            *self.audio_tx.lock() = None;
            return false;
        }
        true
    }

    fn signal_success(&self, transcript: RawTranscript) {
        let tx = self.state.lock().final_tx.take();
        if let Some(tx) = tx {
            let _ = tx.send(Ok(transcript));
        }
    }

    fn signal_error(&self, err: VolcengineASRError) {
        let tx = self.state.lock().final_tx.take();
        if let Some(tx) = tx {
            let _ = tx.send(Err(err));
        }
    }

    /// 服务端 close / 网络中断时调用：如果有缓存的 partial 文本，作为 transcript
    /// 兜底返回；否则才报错。配合 `last_partial_text` 实现「至少不丢用户已识别出的话」。
    fn fallback_to_partial_or_error(&self, err: VolcengineASRError) {
        let (partial, duration_ms) = {
            let st = self.state.lock();
            (
                st.last_partial_text.clone(),
                st.start
                    .map(|s| s.elapsed().as_millis() as u64)
                    .unwrap_or(0),
            )
        };
        if !partial.is_empty() {
            log::warn!(
                "[asr] {}; 使用 partial 兜底（{} 字）",
                err,
                partial.chars().count()
            );
            self.signal_success(RawTranscript {
                text: partial,
                duration_ms,
            });
        } else {
            self.signal_error(err);
        }
        self.state.lock().is_connected = false;
        *self.audio_tx.lock() = None;
    }
}

impl AudioConsumer for VolcengineStreamingASR {
    fn consume_pcm_chunk(&self, pcm: &[u8]) {
        // 单 worker 串行 send 模式：在 state 锁内 drain 并分配 seq（seq 单调），
        // 然后把 (seq, chunk) push 进 mpsc。worker 端按入队顺序 send，
        // 哪怕跨多个 consume 调用、多个 spawn 也不会再有 writer 锁竞争。
        let chunks: Vec<(i32, Vec<u8>)> = {
            let mut st = self.state.lock();
            if !st.is_connected {
                return;
            }
            st.pending_audio.extend_from_slice(pcm);

            let mut out: Vec<(i32, Vec<u8>)> = Vec::new();
            while st.pending_audio.len() >= TARGET_AUDIO_CHUNK_BYTES {
                let chunk: Vec<u8> = st.pending_audio.drain(..TARGET_AUDIO_CHUNK_BYTES).collect();
                let seq = st.next_sequence;
                st.next_sequence += 1;
                st.bytes_sent += chunk.len();
                st.frames_sent += 1;
                out.push((seq, chunk));
            }
            out
        };

        if chunks.is_empty() {
            return;
        }
        let Some(tx) = self.audio_tx.lock().as_ref().cloned() else {
            return;
        };

        for entry in chunks {
            // pending_sends 必须在 tx.send 之前 +1：否则 worker 可能先 recv + 发送 +
            // 减 1，把 usize 计数器 underflow。
            self.pending_sends.fetch_add(1, Ordering::SeqCst);
            if tx.send(entry).is_err() {
                // worker 已退出（cancel / 错误路径里 audio_tx 被 take）。
                // 撤销刚才的 +1，避免 send_last_frame 的 wait 永远等不到 0。
                if self.pending_sends.fetch_sub(1, Ordering::SeqCst) == 1 {
                    self.send_done.notify_waiters();
                }
                log::warn!("[asr] audio queue closed; dropping subsequent frames");
                return;
            }
        }
    }
}

async fn send_binary(writer: &SharedWriter, data: Vec<u8>) -> Result<(), VolcengineASRError> {
    let mut guard = writer.lock().await;
    let Some(sink) = guard.as_mut() else {
        return Err(VolcengineASRError::ConnectionFailed(
            "websocket not open".into(),
        ));
    };
    sink.send(Message::Binary(data))
        .await
        .map_err(|e| VolcengineASRError::ConnectionFailed(e.to_string()))
}

fn hex_prefix(data: &[u8], n: usize) -> String {
    data.iter()
        .take(n)
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join("")
}

fn normalized_result(json: &Value) -> Option<&Value> {
    if let Some(obj) = json.get("result") {
        if obj.is_object() {
            return Some(obj);
        }
        if let Some(arr) = obj.as_array() {
            if let Some(first) = arr.first() {
                return Some(first);
            }
        }
    }
    if json.get("text").and_then(|v| v.as_str()).is_some() {
        return Some(json);
    }
    None
}

/// 把 tokio-tungstenite 的 connect 错误分类：握手收到 HTTP 401 / 403 → `AuthRejected`
/// （凭据被拒，要 user 检查 App ID / Access Token / 账号资源开通状态）；429 →
/// `RateLimited`（请求过多 / 限流，重试只会火上浇油，短路报明确文案）；其它 → 通用
/// `ConnectionFailed`（DNS / TLS / 网络层）。让 capsule 文案能跟泛泛 HTTP error 区分。
fn classify_connect_error(err: tokio_tungstenite::tungstenite::Error) -> VolcengineASRError {
    use tokio_tungstenite::tungstenite::Error as WsError;
    if let WsError::Http(resp) = &err {
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return VolcengineASRError::AuthRejected(status);
        }
        if status == 429 {
            return VolcengineASRError::RateLimited(status);
        }
    }
    VolcengineASRError::ConnectionFailed(err.to_string())
}

/// 握手错误是否「重试也无益」，`connect_with_retry` 据此短路。凭据被拒（401/403）
/// 与限流（429）都属此类：前者重试不会变对，后者重试只会加剧限流。其余（网络层）
/// 才值得在抖动时重试。
fn is_non_retryable(err: &VolcengineASRError) -> bool {
    matches!(
        err,
        VolcengineASRError::AuthRejected(_) | VolcengineASRError::RateLimited(_)
    )
}

/// 热词直传条目：去重（大小写折叠）、跳过停用/空白、封顶 [`HOTWORD_CAP`]。
fn hotword_words(entries: &[DictionaryHotword]) -> Option<Vec<Value>> {
    let mut seen: Vec<String> = Vec::new();
    for entry in entries {
        if !entry.enabled {
            continue;
        }
        let trimmed = entry.phrase.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.iter().any(|w| w.eq_ignore_ascii_case(trimmed)) {
            continue;
        }
        seen.push(trimmed.to_string());
        if seen.len() >= HOTWORD_CAP {
            break;
        }
    }
    if seen.is_empty() {
        return None;
    }
    Some(seen.into_iter().map(|w| json!({ "word": w })).collect())
}

/// dialog_ctx 语境提示条目（调用方按 新→旧 传入）：官方限 800 tokens / 20 轮、
/// 从新到旧截断（docs/6561/1354869）。中文按 1 字符 ≈ 1 token 保守估算，
/// 总字符数超 [`DIALOG_CTX_CHAR_CAP`] 丢弃更旧的条目（保住最近的话）。
/// 固定场景条目（[`DIALOG_CTX_SCENE_ENTRY`]）常驻首位并计入预算——即使没有
/// 最近语音（背景开关关闭/无历史）也注入，让场景偏置对所有人生效。
fn dialog_ctx_entries(lines: &[String]) -> Option<Vec<String>> {
    let mut kept: Vec<String> = vec![DIALOG_CTX_SCENE_ENTRY.to_string()];
    let mut used = DIALOG_CTX_SCENE_ENTRY.chars().count();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if used + trimmed.chars().count() > DIALOG_CTX_CHAR_CAP {
            break;
        }
        used += trimmed.chars().count();
        kept.push(trimmed.to_string());
    }
    Some(kept)
}

/// 首帧 request.context 的组装：热词直传（既有）＋ dialog_ctx 语境提示
/// （批次 A，同一 context 对象内并列）。dialog_ctx 常驻（至少含固定场景
/// 条目），故 context 恒 Some；context_data 条目按官方格式 `{"text": …}`
/// 对象形态下发。沿用既有 string 化 JSON 形态（与线上的热词注入一致）。
fn context_payload(hotwords: &[DictionaryHotword], dialog_ctx: &[String]) -> Option<String> {
    let hotwords = hotword_words(hotwords);
    let dialog_ctx = dialog_ctx_entries(dialog_ctx);
    if hotwords.is_none() && dialog_ctx.is_none() {
        return None;
    }
    let mut object = serde_json::Map::new();
    if let Some(words) = hotwords {
        object.insert("hotwords".into(), Value::Array(words));
    }
    if let Some(entries) = dialog_ctx {
        object.insert("context_type".into(), Value::String("dialog_ctx".into()));
        object.insert(
            "context_data".into(),
            Value::Array(
                entries
                    .into_iter()
                    .map(|text| json!({ "text": text }))
                    .collect(),
            ),
        );
    }
    serde_json::to_string(&Value::Object(object)).ok()
}

/// 输入框文本截断：超长时保留尾部 [`INPUT_BOX_ENTRY_CHAR_CAP`] 字符（光标
/// 附近的最新内容对识别最相关），省略部分以「…」开头；先 trim 再截断。
pub fn truncate_input_box_tail(text: &str) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= INPUT_BOX_ENTRY_CHAR_CAP {
        return trimmed.to_string();
    }
    let mut tail: String = trimmed.chars().skip(count - INPUT_BOX_ENTRY_CHAR_CAP).collect();
    tail.insert(0, '…');
    tail
}

/// dialog_ctx 条目拼装（决策 3 输入框偏置，cloud_providers 调用）：输入框
/// 条目（如有，非空白才收）在固定场景条目之后、最近语音之前；最近语音保持
/// 调用方顺序（新→旧）。返回值整体交 [`dialog_ctx_entries`] 计入 800 字符
/// 预算——输入框条目必然保留，超预算丢的是更旧的最近语音。
pub fn dialog_ctx_lines(input_box: Option<&str>, recent_voice_newest_first: &[String]) -> Vec<String> {
    let mut lines = Vec::with_capacity(recent_voice_newest_first.len() + 1);
    if let Some(text) = input_box.map(str::trim).filter(|text| !text.is_empty()) {
        let mut entry = String::from(DIALOG_CTX_INPUT_BOX_PREFIX);
        entry.push_str(&truncate_input_box_tail(text));
        lines.push(entry);
    }
    lines.extend(recent_voice_newest_first.iter().cloned());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_credentials() -> VolcengineCredentials {
        VolcengineCredentials {
            service: VolcengineService::Standard,
            auth_mode: VolcengineAuthMode::AppIdToken,
            app_id: "app".into(),
            access_token: "secret".into(),
            resource_id: VolcengineCredentials::default_resource_id().into(),
        }
    }

    #[test]
    fn first_frame_enables_nonstream_second_pass_only_when_configured() {
        // 二遍复核（用户裁决 2026-09-18）：官方参数 enable_nonstream（大模型流式
        // 优化版支持，判停分句用非流式模型重识别）。开→显式传 true；关→不传
        // 该键（不是传 false——保持旧行为零歧义）。
        let enabled = VolcengineStreamingASR::new(test_credentials(), vec![], Vec::new(), true);
        let payload = enabled.build_first_frame_payload("connect-id");
        assert_eq!(payload["request"]["enable_nonstream"], Value::Bool(true));

        let disabled = VolcengineStreamingASR::new(test_credentials(), vec![], Vec::new(), false);
        let payload = disabled.build_first_frame_payload("connect-id");
        assert!(
            payload["request"].get("enable_nonstream").is_none(),
            "关闭时不应携带 enable_nonstream 键: {payload}"
        );
    }

    #[test]
    fn hotword_context_dedupes_case_insensitively_and_caps() {
        let mut entries = vec![
            DictionaryHotword {
                phrase: "Foo".into(),
                enabled: true,
            },
            DictionaryHotword {
                phrase: "foo".into(),
                enabled: true,
            },
            DictionaryHotword {
                phrase: "  ".into(),
                enabled: true,
            },
            DictionaryHotword {
                phrase: "Bar".into(),
                enabled: false,
            },
            DictionaryHotword {
                phrase: "Baz".into(),
                enabled: true,
            },
        ];
        for i in 0..200 {
            entries.push(DictionaryHotword {
                phrase: format!("w{}", i),
                enabled: true,
            });
        }
        let payload = context_payload(&entries, &Vec::new()).expect("should produce JSON");
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert!(parsed.get("hotwords").is_some());
        assert_eq!(parsed["hotwords"][0]["word"], "Foo");
        assert_eq!(parsed["hotwords"][1]["word"], "Baz");
        assert!(
            !payload.contains("Bar"),
            "停用热词不应入列: {payload}"
        );
        let count = payload.matches("\"word\"").count();
        // 「word」键计数：热词封顶 80＋场景条目（context_data[].text）。
        assert!(count <= HOTWORD_CAP + 1);
    }

    #[test]
    fn hotword_context_omits_hotwords_key_when_all_disabled() {
        let entries = vec![DictionaryHotword {
            phrase: "Foo".into(),
            enabled: false,
        }];
        let payload = context_payload(&entries, &Vec::new()).expect("场景条目常驻，仍有 context");
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert!(parsed.get("hotwords").is_none(), "全停用不应带 hotwords 键");
        assert_eq!(parsed["context_type"], "dialog_ctx");
        assert_eq!(
            parsed["context_data"][0]["text"],
            DIALOG_CTX_SCENE_ENTRY,
            "无热词无历史时场景条目仍应常驻"
        );
    }

    #[test]
    fn dialog_ctx_scene_entry_is_fixed_constant_and_leads_context_data() {
        // 用户裁决（2026-09-18）：固定场景描述（官方示例 d 类业务/个性化信息），
        // 逐字钉住；在最近语音之前（context_data 首位，官方从新到旧截断时最靠前）。
        assert_eq!(
            DIALOG_CTX_SCENE_ENTRY,
            "用户在通过语音给 AI 助手下指令，内容为待办与任务类表达"
        );
        let payload = context_payload(&[], &vec!["最新一句".into()]).expect("有 dialog_ctx 应产出 context");
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        let texts: Vec<&str> = parsed["context_data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec![DIALOG_CTX_SCENE_ENTRY, "最新一句"]);
    }

    #[test]
    fn context_payload_carries_dialog_ctx_entries_newest_first() {
        // dialog_ctx 语境提示（批次 A）：context_type=dialog_ctx + context_data[]，
        // 条目按调用方顺序（新→旧）原样入列；场景条目固定在首位。
        let payload = context_payload(&[], &vec!["最新一句".into(), "更早一句".into()])
            .expect("有 dialog_ctx 应产出 context");
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["context_type"], "dialog_ctx");
        assert_eq!(
            parsed["context_data"],
            serde_json::json!([
                { "text": DIALOG_CTX_SCENE_ENTRY },
                { "text": "最新一句" },
                { "text": "更早一句" }
            ])
        );
        assert!(parsed.get("hotwords").is_none(), "无热词不应带 hotwords 键");
    }

    #[test]
    fn context_payload_merges_hotwords_and_dialog_ctx() {
        let hotwords = vec![DictionaryHotword {
            phrase: "OpenLess".into(),
            enabled: true,
        }];
        let payload = context_payload(&hotwords, &vec!["最近语音一句".into()])
            .expect("热词＋dialog_ctx 应共存于同一 context");
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["context_type"], "dialog_ctx");
        assert_eq!(
            parsed["context_data"],
            serde_json::json!([
                { "text": DIALOG_CTX_SCENE_ENTRY },
                { "text": "最近语音一句" }
            ])
        );
        assert_eq!(parsed["hotwords"][0]["word"], "OpenLess");
    }

    #[test]
    fn dialog_ctx_entries_cap_total_chars_with_scene_reserved_dropping_oldest() {
        // 官方 dialog_ctx 限 800 tokens：场景条目常驻计入预算，其余条目总字符数
        // 超限丢弃更旧的（保住最新）。
        let long = "y".repeat(500);
        let lines = vec![long.clone(), long.clone(), long.clone()];
        let kept = dialog_ctx_entries(&lines).expect("应保留至少场景条目＋一条");
        assert_eq!(kept.len(), 2, "场景条目＋500：再加一条超 800，只保住最新一条");
        assert_eq!(kept[0], DIALOG_CTX_SCENE_ENTRY);
        assert_eq!(kept[1], long);
        // 预算内（800 chars 减场景条目）原样保留；空白条目跳过。
        let short = vec!["第三条".into(), "  ".into(), "第一条".into()];
        let kept = dialog_ctx_entries(&short).expect("预算内应全留");
        assert_eq!(
            kept,
            vec![
                DIALOG_CTX_SCENE_ENTRY.to_string(),
                "第三条".to_string(),
                "第一条".to_string()
            ]
        );
    }

    #[test]
    fn context_payload_carries_scene_entry_without_history_or_hotwords() {
        // 场景条目常驻：即使无热词、无最近语音，dialog_ctx 也带场景描述
        // （用户裁决 2026-09-18；背景开关默认关时场景偏置仍生效）。
        let payload = context_payload(&[], &Vec::new()).expect("场景条目常驻 → context 恒在");
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["context_type"], "dialog_ctx");
        assert_eq!(parsed["context_data"], serde_json::json!([{ "text": DIALOG_CTX_SCENE_ENTRY }]));
        assert!(parsed.get("hotwords").is_none());
    }

    #[test]
    fn default_resource_id_is_sauc_duration() {
        assert_eq!(
            VolcengineCredentials::default_resource_id(),
            "volc.seedasr.sauc.duration"
        );
    }

    #[test]
    fn resource_id_resolution_defaults_only_missing_or_blank_values() {
        let default_resource_id = "volc.seedasr.sauc.duration";
        let cases = [
            (None, default_resource_id),
            (Some(""), default_resource_id),
            (Some("   "), default_resource_id),
            (Some("\t\r\n"), default_resource_id),
            (
                Some("volc.bigasr.sauc.duration"),
                "volc.bigasr.sauc.duration",
            ),
            (Some(" custom.resource.id "), " custom.resource.id "),
        ];

        for (configured, expected) in cases {
            assert_eq!(
                VolcengineCredentials::resolve_resource_id(configured.map(str::to_string)),
                expected
            );
        }
    }

    #[test]
    fn auth_mode_from_str_roundtrips() {
        assert_eq!(
            VolcengineAuthMode::parse("api_key"),
            VolcengineAuthMode::ApiKey
        );
        assert_eq!(
            VolcengineAuthMode::parse("app_id_token"),
            VolcengineAuthMode::AppIdToken
        );
        assert_eq!(
            VolcengineAuthMode::parse(""),
            VolcengineAuthMode::AppIdToken
        ); // 默认回退
        assert_eq!(VolcengineAuthMode::ApiKey.as_str(), "api_key");
        assert_eq!(VolcengineAuthMode::AppIdToken.as_str(), "app_id_token");
    }

    #[test]
    fn auth_ok_matches_mode_requirements() {
        let app_id_token = VolcengineAuthMode::AppIdToken;
        let api_key = VolcengineAuthMode::ApiKey;
        // AppIdToken：需要 app_id + secret 都非空。
        assert!(app_id_token.auth_ok("app", "token"));
        assert!(!app_id_token.auth_ok("", "token"));
        assert!(!app_id_token.auth_ok("app", ""));
        // 全空格视为未配置（统一 trim 语义，与 volcengine_configured / 预检一致）。
        assert!(!app_id_token.auth_ok("   ", "   "));
        // ApiKey：只需 API Key，app_id 可为空。
        assert!(api_key.auth_ok("", "key"));
        assert!(api_key.auth_ok("app", "key"));
        assert!(!api_key.auth_ok("app", ""));
        assert!(!api_key.auth_ok("", "   "));
    }

    #[test]
    fn build_connect_request_selects_endpoint_and_headers_per_mode() {
        let cases = [
            (
                VolcengineService::Standard,
                VolcengineAuthMode::AppIdToken,
                ENDPOINT_APP_ID_TOKEN,
                true,  // 双表头（X-Api-App-Key / X-Api-Access-Key）
                false, // 不应带 X-Api-Key
            ),
            (
                VolcengineService::Standard,
                VolcengineAuthMode::ApiKey,
                ENDPOINT_API_KEY,
                false, // 不应带双表头
                true,  // 单表头 X-Api-Key
            ),
            (
                VolcengineService::AgentPlan,
                VolcengineAuthMode::AppIdToken,
                ENDPOINT_AGENT_PLAN,
                false,
                true,
            ),
            (
                VolcengineService::AgentPlan,
                VolcengineAuthMode::ApiKey,
                ENDPOINT_AGENT_PLAN,
                false,
                true,
            ),
        ];
        for (service, mode, endpoint, expects_app_headers, expects_api_key) in cases {
            let asr = VolcengineStreamingASR::new(
                VolcengineCredentials {
                    service,
                    auth_mode: mode.clone(),
                    app_id: "app".into(),
                    access_token: "secret".into(),
                    resource_id: VolcengineCredentials::default_resource_id().into(),
                },
                vec![],
                Vec::new(), true,
            );
            let req = asr
                .build_connect_request("connect-id", "request-id")
                .unwrap();
            assert_eq!(
                req.uri().to_string(),
                endpoint,
                "端点应随鉴权模式切换（mode={mode:?}）"
            );
            let headers = req.headers();
            assert_eq!(
                headers.contains_key("X-Api-App-Key"),
                expects_app_headers,
                "mode={mode:?} X-Api-App-Key"
            );
            assert_eq!(
                headers.contains_key("X-Api-Access-Key"),
                expects_app_headers,
                "mode={mode:?} X-Api-Access-Key"
            );
            assert_eq!(
                headers.contains_key("X-Api-Key"),
                expects_api_key,
                "mode={mode:?} X-Api-Key"
            );
            // 两种模式都必须携带资源与连接标识头。
            assert!(headers.contains_key("X-Api-Resource-Id"));
            assert_eq!(headers.get("X-Api-Connect-Id").unwrap(), "connect-id");
            // 官方鉴权表要求的其余头（docs/6561/1354869）。
            assert_eq!(headers.get("X-Api-Request-Id").unwrap(), "request-id");
            assert_ne!(
                headers.get("X-Api-Request-Id"),
                headers.get("X-Api-Connect-Id"),
                "任务 ID 不应复用会话连接 ID"
            );
            assert_eq!(headers.get("X-Api-Sequence").unwrap(), "-1");
        }
        // 回归：新旧两种鉴权模式共享同一官方端点（docs/6561/1354869），
        // 曾因 ApiKey 模式误用 /api/v3/plan/... 路径导致 45000010 AuthenticationError。
        assert_eq!(ENDPOINT_API_KEY, ENDPOINT_APP_ID_TOKEN);
        assert_eq!(
            ENDPOINT_API_KEY,
            "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async"
        );
    }

    /// 构造一个握手阶段返回给定 HTTP 状态码的 tungstenite 错误，用于分类测试。
    fn http_ws_error(status: u16) -> tokio_tungstenite::tungstenite::Error {
        use tokio_tungstenite::tungstenite::http::Response;
        let resp = Response::builder()
            .status(status)
            .body(None)
            .expect("build test http response");
        tokio_tungstenite::tungstenite::Error::Http(resp)
    }

    #[test]
    fn classify_429_is_rate_limited_not_connection_failed() {
        let classified = classify_connect_error(http_ws_error(429));
        assert!(
            matches!(classified, VolcengineASRError::RateLimited(429)),
            "429 应归类为 RateLimited 而非 ConnectionFailed，避免被当网络抖动重试"
        );
    }

    #[test]
    fn rate_limited_is_non_retryable_like_auth_rejected() {
        assert!(
            is_non_retryable(&VolcengineASRError::RateLimited(429)),
            "限流不可重试：重试只会加剧限流"
        );
        assert!(
            is_non_retryable(&VolcengineASRError::AuthRejected(401)),
            "凭据被拒不可重试"
        );
    }

    #[test]
    fn network_errors_stay_retryable() {
        // 通用网络失败仍应重试（抖动可恢复）——不能被误判成短路。
        assert!(!is_non_retryable(&VolcengineASRError::ConnectionFailed(
            "dns fail".into()
        )));
        assert!(!is_non_retryable(&VolcengineASRError::FinalResultTimeout));
    }

    #[test]
    fn classify_401_403_still_auth_rejected() {
        // 回归：新增 429 分类不影响既有 401 / 403 → AuthRejected。
        assert!(matches!(
            classify_connect_error(http_ws_error(401)),
            VolcengineASRError::AuthRejected(401)
        ));
        assert!(matches!(
            classify_connect_error(http_ws_error(403)),
            VolcengineASRError::AuthRejected(403)
        ));
    }

    #[test]
    fn rate_limited_message_mentions_throttling_not_network() {
        // 文案必须明确指向「限流/请求过多」，不是含糊的「网络失败」。
        let msg = VolcengineASRError::RateLimited(429).to_string();
        assert!(
            msg.contains("限流") || msg.contains("请求过多"),
            "文案: {msg}"
        );
        assert!(!msg.contains("网络"), "限流文案不应误导为网络失败: {msg}");
    }

    #[tokio::test]
    async fn await_final_result_returns_error_when_final_frame_never_arrives() {
        let asr = VolcengineStreamingASR::new(
            VolcengineCredentials {
                service: VolcengineService::Standard,
                auth_mode: VolcengineAuthMode::AppIdToken,
                app_id: "app".into(),
                access_token: "token".into(),
                resource_id: VolcengineCredentials::default_resource_id().into(),
            },
            Vec::new(),
            Vec::new(), true,
        );
        let (tx, rx) = oneshot::channel();
        asr.state.lock().final_tx = Some(tx);
        *asr.final_rx.lock() = Some(rx);

        let result = asr
            .await_final_result_with_timeout(std::time::Duration::from_millis(10))
            .await;

        assert!(matches!(
            result,
            Err(VolcengineASRError::FinalResultTimeout)
        ));
    }

    #[test]
    fn truncate_input_box_tail_keeps_tail_and_marks_omission() {
        // 决策 3 输入框偏置：超长文本保留尾部（光标附近最新内容最相关），
        // 截断补「…」；未超长原样返回；首尾空白 trim。
        assert_eq!(truncate_input_box_tail("  草稿内容  "), "草稿内容");
        let long = "a".repeat(INPUT_BOX_ENTRY_CHAR_CAP + 60);
        let truncated = truncate_input_box_tail(&long);
        assert_eq!(
            truncated.chars().count(),
            INPUT_BOX_ENTRY_CHAR_CAP + 1,
            "尾部 400 字符＋省略号"
        );
        assert!(truncated.starts_with('…'));
        assert!(truncated.chars().skip(1).all(|c| c == 'a'));
        let exactly = "b".repeat(INPUT_BOX_ENTRY_CHAR_CAP);
        assert_eq!(truncate_input_box_tail(&exactly), exactly);
    }

    #[test]
    fn dialog_ctx_lines_places_input_box_between_scene_and_recent_voice() {
        // 条目顺序（用户裁决）：固定场景 → 输入框已有内容 → 最近语音（新→旧）。
        let lines = dialog_ctx_lines(Some("帮我看看这段草稿"), &["最新一句".into(), "更早一句".into()]);
        let payload = context_payload(&[], &lines).expect("有 dialog_ctx 应产出 context");
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        let texts: Vec<&str> = parsed["context_data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            texts,
            vec![
                DIALOG_CTX_SCENE_ENTRY,
                "输入框已有内容：帮我看看这段草稿",
                "最新一句",
                "更早一句"
            ]
        );
    }

    #[test]
    fn blank_input_box_yields_no_entry() {
        // 空白输入框（无值/空白）＝没有条目，其余不变。
        assert!(dialog_ctx_lines(None, &[]).is_empty());
        assert!(dialog_ctx_lines(Some("   \n\t "), &[]).is_empty());
    }

    #[test]
    fn long_input_box_text_is_truncated_in_entry_and_counts_into_budget() {
        // 截断防超长：条目总长 ≤ 前缀＋400＋省略号；且计入 800 预算——
        // 输入框条目必然保留（在场景之后），超预算丢的是更旧的最近语音。
        let long = "x".repeat(INPUT_BOX_ENTRY_CHAR_CAP + 500);
        let lines = dialog_ctx_lines(Some(&long), &[long.clone()]);
        assert_eq!(lines.len(), 2);
        let entry = &lines[0];
        assert_eq!(
            entry.chars().count(),
            DIALOG_CTX_INPUT_BOX_PREFIX.chars().count() + INPUT_BOX_ENTRY_CHAR_CAP + 1
        );
        // 拼进 dialog_ctx_entries：场景＋截断输入框条目在预算内常驻，
        // 同一条最近语音（401＋500 字符）超预算被丢弃。
        let payload = context_payload(&[], &lines).expect("有 dialog_ctx 应产出 context");
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        let texts: Vec<&str> = parsed["context_data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            texts,
            vec![DIALOG_CTX_SCENE_ENTRY, entry.as_str()],
            "超预算只丢最近语音，输入框条目与场景常驻"
        );
    }
}
