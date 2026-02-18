---
name: screen-analysis
description: "How to capture and analyze on-screen windows. Refer to this skill when asked to look at, read, describe, or inspect anything on screen."
---
You have three screen analysis tools. Use them in the order below for efficiency.

## Tools

### find_window
Search for windows by keywords. Always start here — don't guess window titles.
- `keywords`: space-delimited, case-insensitive, all must match
- Returns matching windows with title, app name, and size

### capture_screen
Capture a window screenshot, run OCR, or detect objects.

Parameters (all optional, but at least one mode required):
- `window_name`: window title substring match (from find_window results)
- `process_name`: app/process name (e.g. "Safari", "Terminal")
- `crop_x`, `crop_y`, `crop_w`, `crop_h`: normalized 0.0–1.0, crop region
- `ocr`: if true, extract text with bounding boxes (no image returned)
- `detect`: if true, detect objects — text regions, rectangles, faces, barcodes (no image returned)

Modes:
1. **Capture** (window_name or process_name): takes screenshot, caches full image
2. **Crop** (crop fields only): crops from cached image, no re-capture
3. **OCR** (ocr=true): extracts text with positions
4. **Detect** (detect=true): finds text regions, rectangles, faces, barcodes with bounding boxes
5. **Image** (no ocr/detect): returns screenshot for vision analysis

Modes can combine: `window_name` + `ocr` captures and OCRs in one call. `ocr` + `detect` returns both. Crop fields apply to any mode.

## Image Cache

After capturing a window, the full image is cached. Subsequent calls can crop, OCR, or detect from the cache without re-capturing — just omit window_name/process_name. Cache expires after 5 tool calls. The tool description tells you when a cached image is available.

## Recommended Workflow

### Quick text reading
```
find_window → capture_screen(window_name, ocr=true)
```

### Describe what's on screen
```
find_window → capture_screen(window_name)  [returns image for vision]
```

### Efficient deep analysis
```
find_window
→ capture_screen(window_name, detect=true)     [map the layout]
→ capture_screen(crop_x/y/w/h, ocr=true)       [read specific text regions]
```
This is cheaper than OCR on the full image — detect first to find text regions, then crop+OCR only where needed.

### Zoom into details
```
capture_screen(window_name)           [full screenshot, cached]
→ capture_screen(crop_x/y/w/h)       [zoom into area from cache]
```

## Bounding Box Coordinates

All coordinates are normalized 0.0–1.0 (fraction of image width/height), origin at top-left. OCR and detect results include bounding boxes you can use directly as crop_x/y/w/h to zoom in.

## Tips

- Use `detect=true` before `ocr=true` for large windows — it shows where text is, so you can crop+OCR specific areas instead of OCR-ing the entire window
- Barcode detection includes payload content — you get the QR code data without vision
- When the user asks to "look at" or "check" something on screen, start with find_window
- Prefer OCR over vision (image) for text-heavy content — it's much cheaper
- Use vision (image mode) when you need to understand layout, colors, or non-text content
