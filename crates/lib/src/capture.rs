use crossbeam::channel::{self, Receiver, Sender};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::llm::ImageContent;
use crate::tool::{ToolHandler, ToolResult};
use crate::AgentError;

/// Maximum tool calls before cached image is discarded.
const CACHE_MAX_CALLS: u64 = 5;

/// Request to capture a screen window (Rust → Swift)
pub struct CaptureRequest {
    pub id: String,
    pub window_name: Option<String>,
    pub process_name: Option<String>,
    /// Crop region (normalized 0.0–1.0). All four must be set for cropping.
    pub crop_x: Option<f64>,
    pub crop_y: Option<f64>,
    pub crop_w: Option<f64>,
    pub crop_h: Option<f64>,
    /// If true, run OCR and return text instead of image.
    pub ocr: Option<bool>,
    /// If true, run object detection (rectangles, faces, barcodes) and return text.
    pub detect: Option<bool>,
    /// Space-delimited keywords for window search (used by find_window tool).
    pub search_keywords: Option<String>,
}

/// Result of a screen capture (Swift → Rust)
pub struct CaptureResult {
    pub id: String,
    pub image_base64: String,
    pub metadata_json: String,
}

/// Channel pairs for the capture bridge.
/// Both CaptureScreenTool and FindWindowTool share the request channel
/// (Swift polls a single drain), but each has its own result channel
/// so responses are routed to the correct blocking tool.
pub struct CaptureBridge {
    pub request_tx: Sender<CaptureRequest>,
    pub request_rx: Receiver<CaptureRequest>,
    // capture_screen result channel
    pub capture_result_tx: Sender<CaptureResult>,
    pub capture_result_rx: Receiver<CaptureResult>,
    // find_window result channel
    pub find_result_tx: Sender<CaptureResult>,
    pub find_result_rx: Receiver<CaptureResult>,
}

impl CaptureBridge {
    pub fn new() -> Self {
        let (request_tx, request_rx) = channel::unbounded();
        let (capture_result_tx, capture_result_rx) = channel::unbounded();
        let (find_result_tx, find_result_rx) = channel::unbounded();
        Self {
            request_tx,
            request_rx,
            capture_result_tx,
            capture_result_rx,
            find_result_tx,
            find_result_rx,
        }
    }
}

/// Metadata about a cached capture (Rust side tracks this for dynamic_description).
struct CacheInfo {
    metadata: String,
}

/// Tool that captures a specific window by name or process, with optional crop/zoom.
pub struct CaptureScreenTool {
    request_tx: Sender<CaptureRequest>,
    result_rx: Receiver<CaptureResult>,
    next_id: AtomicU64,
    cache: Mutex<Option<CacheInfo>>,
    calls_since_capture: AtomicU64,
}

impl CaptureScreenTool {
    pub fn new(request_tx: Sender<CaptureRequest>, result_rx: Receiver<CaptureResult>) -> Self {
        Self {
            request_tx,
            result_rx,
            next_id: AtomicU64::new(1),
            cache: Mutex::new(None),
            calls_since_capture: AtomicU64::new(0),
        }
    }

    fn has_crop(args: &serde_json::Value) -> bool {
        args.get("crop_x").and_then(|v| v.as_f64()).is_some()
            || args.get("crop_y").and_then(|v| v.as_f64()).is_some()
            || args.get("crop_w").and_then(|v| v.as_f64()).is_some()
            || args.get("crop_h").and_then(|v| v.as_f64()).is_some()
    }
}

impl ToolHandler for CaptureScreenTool {
    fn name(&self) -> &str {
        "capture_screen"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of a window by title or app name. \
         Optionally crop a region using normalized coordinates (0.0–1.0). \
         Omit window_name/process_name to crop/OCR/detect the cached last capture. \
         Set ocr=true to extract text via OCR. \
         Set detect=true to detect objects (text regions, rectangles, faces, barcodes) with bounding boxes. Use text regions to decide where to OCR."
    }

    fn dynamic_description(&self) -> Option<String> {
        let calls = self.calls_since_capture.load(Ordering::Relaxed);
        let guard = self.cache.lock().unwrap();
        if let Some(ref info) = *guard {
            if calls <= CACHE_MAX_CALLS {
                return Some(format!(
                    "{}. [Cached image: {}. Use crop_x/y/w/h to zoom, ocr=true for text, or detect=true for objects, without re-capturing.]",
                    self.description(),
                    info.metadata,
                ));
            }
        }
        None
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "window_name": {
                    "type": "string",
                    "description": "Window title to capture (substring match)"
                },
                "process_name": {
                    "type": "string",
                    "description": "Application/process name to capture (e.g. 'Terminal', 'Safari')"
                },
                "crop_x": {
                    "type": "number",
                    "description": "Left edge of crop region (0.0–1.0, normalized to image width)"
                },
                "crop_y": {
                    "type": "number",
                    "description": "Top edge of crop region (0.0–1.0, normalized to image height)"
                },
                "crop_w": {
                    "type": "number",
                    "description": "Width of crop region (0.0–1.0, normalized to image width)"
                },
                "crop_h": {
                    "type": "number",
                    "description": "Height of crop region (0.0–1.0, normalized to image height)"
                },
                "ocr": {
                    "type": "boolean",
                    "description": "If true, run OCR and return extracted text with bounding boxes instead of the image. Much cheaper than vision."
                },
                "detect": {
                    "type": "boolean",
                    "description": "If true, detect objects (text regions, rectangles, faces, barcodes) and return bounding boxes. Text regions show where text is — crop and OCR to read it."
                }
            }
        })
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let window_name = args.get("window_name").and_then(|v| v.as_str()).map(String::from);
        let process_name = args.get("process_name").and_then(|v| v.as_str()).map(String::from);
        let crop_x = args.get("crop_x").and_then(|v| v.as_f64());
        let crop_y = args.get("crop_y").and_then(|v| v.as_f64());
        let crop_w = args.get("crop_w").and_then(|v| v.as_f64());
        let crop_h = args.get("crop_h").and_then(|v| v.as_f64());
        let ocr = args.get("ocr").and_then(|v| v.as_bool());
        let detect = args.get("detect").and_then(|v| v.as_bool());

        let is_capture = window_name.is_some() || process_name.is_some();
        let is_crop = Self::has_crop(&args);
        let is_ocr = ocr == Some(true);
        let is_detect = detect == Some(true);

        if !is_capture && !is_crop && !is_ocr && !is_detect {
            return Err(AgentError::ParseError(
                "Either window_name/process_name (to capture), crop_x/y/w/h (to crop), ocr=true, or detect=true must be specified".to_string(),
            ));
        }

        // Expire cache if too many calls since last capture
        let calls = self.calls_since_capture.fetch_add(1, Ordering::SeqCst);
        if calls > CACHE_MAX_CALLS {
            *self.cache.lock().unwrap() = None;
        }

        // Non-capture mode (crop/OCR/detect on cached image) without cache → error
        if !is_capture && (is_crop || is_ocr || is_detect) {
            let has_cache = self.cache.lock().unwrap().is_some();
            if !has_cache {
                return Err(AgentError::ParseError(
                    "No cached image. Capture a window first by specifying window_name or process_name.".to_string(),
                ));
            }
        }

        let id = format!("cap_{}", self.next_id.fetch_add(1, Ordering::SeqCst));

        let request = CaptureRequest {
            id: id.clone(),
            window_name,
            process_name,
            crop_x,
            crop_y,
            crop_w,
            crop_h,
            ocr,
            detect,
            search_keywords: None,
        };

        self.request_tx.send(request).map_err(|e| {
            AgentError::InternalError(format!("Failed to send capture request: {}", e))
        })?;

        // Block waiting for the result with 10s timeout
        let result = self
            .result_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|e| {
                AgentError::InternalError(format!("Capture timeout or error: {}", e))
            })?;

        // Update cache on successful capture (even if OCR-only — Swift caches the image)
        if is_capture && !result.metadata_json.starts_with("Error") {
            *self.cache.lock().unwrap() = Some(CacheInfo {
                metadata: result.metadata_json.clone(),
            });
            self.calls_since_capture.store(0, Ordering::SeqCst);
        }

        if result.image_base64.is_empty() {
            // OCR-only or error: text result, no image
            return Ok(ToolResult::text(result.metadata_json));
        }

        Ok(ToolResult::with_images(
            result.metadata_json,
            vec![ImageContent {
                base64: result.image_base64,
                media_type: "image/png".to_string(),
            }],
        ))
    }
}

/// Tool that searches for windows by space-delimited keywords.
pub struct FindWindowTool {
    request_tx: Sender<CaptureRequest>,
    result_rx: Receiver<CaptureResult>,
    next_id: AtomicU64,
}

impl FindWindowTool {
    pub fn new(request_tx: Sender<CaptureRequest>, result_rx: Receiver<CaptureResult>) -> Self {
        Self {
            request_tx,
            result_rx,
            next_id: AtomicU64::new(1),
        }
    }
}

impl ToolHandler for FindWindowTool {
    fn name(&self) -> &str {
        "find_window"
    }

    fn description(&self) -> &str {
        "Search for windows by keywords. Returns matching windows with title, app name, and size. \
         Use this to discover exact window names before calling capture_screen."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "keywords": {
                    "type": "string",
                    "description": "Space-delimited keywords to match against window title and app name (case-insensitive, all keywords must match)"
                }
            },
            "required": ["keywords"]
        })
    }

    fn call(&self, args: serde_json::Value) -> Result<ToolResult, AgentError> {
        let keywords = args
            .get("keywords")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentError::ParseError("'keywords' parameter is required".to_string()))?;

        if keywords.trim().is_empty() {
            return Err(AgentError::ParseError("'keywords' must not be empty".to_string()));
        }

        let id = format!("find_{}", self.next_id.fetch_add(1, Ordering::SeqCst));

        let request = CaptureRequest {
            id: id.clone(),
            window_name: None,
            process_name: None,
            crop_x: None,
            crop_y: None,
            crop_w: None,
            crop_h: None,
            ocr: None,
            detect: None,
            search_keywords: Some(keywords.to_string()),
        };

        self.request_tx.send(request).map_err(|e| {
            AgentError::InternalError(format!("Failed to send find_window request: {}", e))
        })?;

        let result = self
            .result_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|e| {
                AgentError::InternalError(format!("find_window timeout or error: {}", e))
            })?;

        Ok(ToolResult::text(result.metadata_json))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: spawn a thread that responds to capture requests
    fn mock_swift_side(
        request_rx: Receiver<CaptureRequest>,
        result_tx: Sender<CaptureResult>,
        image: &str,
        metadata: &str,
    ) {
        let image = image.to_string();
        let metadata = metadata.to_string();
        std::thread::spawn(move || {
            let req = request_rx.recv().unwrap();
            result_tx
                .send(CaptureResult {
                    id: req.id,
                    image_base64: image,
                    metadata_json: metadata,
                })
                .unwrap();
        });
    }

    #[test]
    fn test_capture_bridge_round_trip() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        mock_swift_side(
            bridge.request_rx.clone(),
            bridge.capture_result_tx.clone(),
            "iVBORw0KGgo=",
            "Window: Terminal, Size: 800x600",
        );

        let result = tool
            .call(serde_json::json!({"window_name": "Terminal"}))
            .unwrap();
        assert!(result.text.contains("Terminal"));
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0].media_type, "image/png");
    }

    #[test]
    fn test_capture_missing_args() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        let err = tool.call(serde_json::json!({})).unwrap_err();
        assert!(err.to_string().contains("window_name"));
    }

    #[test]
    fn test_capture_error_result() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        mock_swift_side(
            bridge.request_rx.clone(),
            bridge.capture_result_tx.clone(),
            "",
            "Error: window not found",
        );

        let result = tool
            .call(serde_json::json!({"process_name": "Nonexistent"}))
            .unwrap();
        assert!(result.text.contains("Error"));
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_capture_sends_crop_fields() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        let request_rx = bridge.request_rx.clone();
        let result_tx = bridge.capture_result_tx.clone();
        std::thread::spawn(move || {
            let req = request_rx.recv().unwrap();
            // Verify crop fields were passed through
            assert_eq!(req.crop_x, Some(0.1));
            assert_eq!(req.crop_y, Some(0.2));
            assert_eq!(req.crop_w, Some(0.5));
            assert_eq!(req.crop_h, Some(0.5));
            result_tx
                .send(CaptureResult {
                    id: req.id,
                    image_base64: "CROPPED".to_string(),
                    metadata_json: "Window: Chrome, Cropped: 0.1,0.2 50%x50%".to_string(),
                })
                .unwrap();
        });

        let result = tool
            .call(serde_json::json!({
                "process_name": "Chrome",
                "crop_x": 0.1, "crop_y": 0.2,
                "crop_w": 0.5, "crop_h": 0.5,
            }))
            .unwrap();
        assert!(result.text.contains("Cropped"));
        assert_eq!(result.images.len(), 1);
    }

    #[test]
    fn test_crop_only_without_cache_fails() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        let err = tool
            .call(serde_json::json!({"crop_x": 0.0, "crop_y": 0.0, "crop_w": 0.5, "crop_h": 0.5}))
            .unwrap_err();
        assert!(err.to_string().contains("No cached image"));
    }

    #[test]
    fn test_crop_only_with_cache_succeeds() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        // First: capture to populate cache
        {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: "FULL_IMAGE".to_string(),
                    metadata_json: "Window: Chrome, Size: 1920x1080".to_string(),
                })
                .unwrap();
            });
            tool.call(serde_json::json!({"process_name": "Chrome"}))
                .unwrap();
        }

        // Second: crop-only from cache (still goes through channel to Swift)
        {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                assert!(req.window_name.is_none());
                assert!(req.process_name.is_none());
                assert_eq!(req.crop_x, Some(0.0));
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: "CROPPED".to_string(),
                    metadata_json: "Cropped from cache".to_string(),
                })
                .unwrap();
            });
            let result = tool
                .call(serde_json::json!({"crop_x": 0.0, "crop_y": 0.0, "crop_w": 0.5, "crop_h": 0.5}))
                .unwrap();
            assert_eq!(result.images.len(), 1);
        }
    }

    #[test]
    fn test_cache_expires_after_max_calls() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        // Capture to populate cache
        {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: "IMG".to_string(),
                    metadata_json: "Window: X".to_string(),
                })
                .unwrap();
            });
            tool.call(serde_json::json!({"process_name": "X"})).unwrap();
        }

        // Make CACHE_MAX_CALLS + 1 crop-only calls to expire cache
        // (crop-only calls don't reset the counter)
        for _ in 0..=CACHE_MAX_CALLS {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: "CROP".to_string(),
                    metadata_json: "Cropped".to_string(),
                })
                .unwrap();
            });
            tool.call(serde_json::json!({"crop_x": 0.0, "crop_y": 0.0, "crop_w": 1.0, "crop_h": 1.0}))
                .unwrap();
        }

        // Now crop-only should fail (cache expired)
        let err = tool
            .call(serde_json::json!({"crop_x": 0.0, "crop_y": 0.0, "crop_w": 1.0, "crop_h": 1.0}))
            .unwrap_err();
        assert!(err.to_string().contains("No cached image"));
    }

    #[test]
    fn test_dynamic_description_with_cache() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        // No cache → None
        assert!(tool.dynamic_description().is_none());

        // Capture to populate cache
        {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: "IMG".to_string(),
                    metadata_json: "Window: Chrome, Size: 1920x1080".to_string(),
                })
                .unwrap();
            });
            tool.call(serde_json::json!({"process_name": "Chrome"}))
                .unwrap();
        }

        // Now dynamic_description should include cache info
        let desc = tool.dynamic_description().unwrap();
        assert!(desc.contains("Cached image"));
        assert!(desc.contains("Chrome"));
        assert!(desc.contains("crop_x"));
    }

    #[test]
    fn test_ocr_capture_returns_text_only() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        let rx = bridge.request_rx.clone();
        let tx = bridge.capture_result_tx.clone();
        std::thread::spawn(move || {
            let req = rx.recv().unwrap();
            assert_eq!(req.ocr, Some(true));
            // Swift side returns OCR text with empty image_base64
            tx.send(CaptureResult {
                id: req.id,
                image_base64: String::new(),
                metadata_json: "OCR Results (3 entries):\n  [0.1,0.05] \"Hello\" (98%)\n  [0.1,0.10] \"World\" (95%)".to_string(),
            })
            .unwrap();
        });

        let result = tool
            .call(serde_json::json!({"process_name": "Chrome", "ocr": true}))
            .unwrap();
        assert!(result.text.contains("OCR Results"));
        assert!(result.text.contains("Hello"));
        assert!(result.images.is_empty(), "OCR should return text only, no images");
    }

    #[test]
    fn test_ocr_capture_still_caches() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        // Capture with OCR
        {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: String::new(),
                    metadata_json: "Window: Chrome\nOCR: some text".to_string(),
                })
                .unwrap();
            });
            tool.call(serde_json::json!({"process_name": "Chrome", "ocr": true}))
                .unwrap();
        }

        // Cache should be populated (capture happened)
        let desc = tool.dynamic_description().unwrap();
        assert!(desc.contains("Cached image"));

        // Crop from cache should work
        {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                assert!(req.window_name.is_none());
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: "CROPPED".to_string(),
                    metadata_json: "Cropped from cache".to_string(),
                })
                .unwrap();
            });
            let result = tool
                .call(serde_json::json!({"crop_x": 0.0, "crop_y": 0.0, "crop_w": 0.5, "crop_h": 0.5}))
                .unwrap();
            assert_eq!(result.images.len(), 1);
        }
    }

    #[test]
    fn test_ocr_only_from_cache() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        // First capture to populate cache
        {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: "IMG".to_string(),
                    metadata_json: "Window: Chrome".to_string(),
                })
                .unwrap();
            });
            tool.call(serde_json::json!({"process_name": "Chrome"})).unwrap();
        }

        // OCR-only from cache (no window_name/process_name)
        {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                assert!(req.window_name.is_none());
                assert!(req.process_name.is_none());
                assert_eq!(req.ocr, Some(true));
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: String::new(),
                    metadata_json: "OCR: cached text".to_string(),
                })
                .unwrap();
            });
            let result = tool
                .call(serde_json::json!({"ocr": true}))
                .unwrap();
            assert!(result.text.contains("OCR"));
            assert!(result.images.is_empty());
        }
    }

    #[test]
    fn test_ocr_only_without_cache_fails() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        let err = tool.call(serde_json::json!({"ocr": true})).unwrap_err();
        assert!(err.to_string().contains("No cached image"));
    }

    // ---- Object detection tests ----

    #[test]
    fn test_detect_capture_returns_text_only() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        let rx = bridge.request_rx.clone();
        let tx = bridge.capture_result_tx.clone();
        std::thread::spawn(move || {
            let req = rx.recv().unwrap();
            assert_eq!(req.detect, Some(true));
            tx.send(CaptureResult {
                id: req.id,
                image_base64: String::new(),
                metadata_json: "Object Detection (3 objects):\n  rectangles (2):\n    [0.1,0.1 50%x30%] 85%\n  faces (1):\n    [0.3,0.2 20%x25%] 92%".to_string(),
            }).unwrap();
        });

        let result = tool
            .call(serde_json::json!({"process_name": "Chrome", "detect": true}))
            .unwrap();
        assert!(result.text.contains("Object Detection"));
        assert!(result.text.contains("rectangles"));
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_detect_only_without_cache_fails() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        let err = tool.call(serde_json::json!({"detect": true})).unwrap_err();
        assert!(err.to_string().contains("No cached image"));
    }

    #[test]
    fn test_detect_from_cache() {
        let bridge = CaptureBridge::new();
        let tool = CaptureScreenTool::new(bridge.request_tx.clone(), bridge.capture_result_rx.clone());

        // First capture to populate cache
        {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: "IMG".to_string(),
                    metadata_json: "Window: Safari".to_string(),
                }).unwrap();
            });
            tool.call(serde_json::json!({"process_name": "Safari"})).unwrap();
        }

        // Detect from cache
        {
            let rx = bridge.request_rx.clone();
            let tx = bridge.capture_result_tx.clone();
            std::thread::spawn(move || {
                let req = rx.recv().unwrap();
                assert!(req.window_name.is_none());
                assert_eq!(req.detect, Some(true));
                tx.send(CaptureResult {
                    id: req.id,
                    image_base64: String::new(),
                    metadata_json: "Object Detection (1 objects):\n  barcodes (1):\n    [0.5,0.5 10%x10%] 99% payload=\"https://example.com\"".to_string(),
                }).unwrap();
            });
            let result = tool.call(serde_json::json!({"detect": true})).unwrap();
            assert!(result.text.contains("barcodes"));
            assert!(result.images.is_empty());
        }
    }

    // ---- FindWindowTool tests ----

    #[test]
    fn test_find_window_round_trip() {
        let bridge = CaptureBridge::new();
        let tool = FindWindowTool::new(bridge.request_tx.clone(), bridge.find_result_rx.clone());

        let rx = bridge.request_rx.clone();
        let tx = bridge.find_result_tx.clone();
        std::thread::spawn(move || {
            let req = rx.recv().unwrap();
            assert!(req.id.starts_with("find_"));
            assert_eq!(req.search_keywords, Some("terminal code".to_string()));
            tx.send(CaptureResult {
                id: req.id,
                image_base64: String::new(),
                metadata_json: "Found 2 window(s):\n  Terminal — zsh (80x24)\n  Code — main.rs (1200x800)".to_string(),
            }).unwrap();
        });

        let result = tool.call(serde_json::json!({"keywords": "terminal code"})).unwrap();
        assert!(result.text.contains("Found 2"));
        assert!(result.images.is_empty());
    }

    #[test]
    fn test_find_window_missing_keywords() {
        let bridge = CaptureBridge::new();
        let tool = FindWindowTool::new(bridge.request_tx.clone(), bridge.find_result_rx.clone());

        let err = tool.call(serde_json::json!({})).unwrap_err();
        assert!(err.to_string().contains("keywords"));
    }

    #[test]
    fn test_find_window_empty_keywords() {
        let bridge = CaptureBridge::new();
        let tool = FindWindowTool::new(bridge.request_tx.clone(), bridge.find_result_rx.clone());

        let err = tool.call(serde_json::json!({"keywords": "  "})).unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }
}
