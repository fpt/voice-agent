---
name: screen-analysis
description: "How to capture and analyze on-screen windows. Refer to this skill when asked to look at, read, describe, or inspect anything on screen."
---
**Capture first, analyze later.** Image is cached after capture — use apply_ocr or detect from cache.

## Tools

### find_window
Search for windows by keywords. Returns window IDs.
- `keywords`: space-delimited, case-insensitive, all must match
- Returns: id, title, app name, size for each match

### capture_screen
Capture a window by ID (from find_window). Returns screenshot image.
- `window_id` (required): window ID from find_window
- `detect`: return object/text bounding boxes instead of image

### apply_ocr
Run OCR on the cached captured image. Returns text with bounding boxes.
- `crop_x`, `crop_y`, `crop_w`, `crop_h`: optional region (0.0–1.0)

## Workflow

**Always capture immediately** — never ask the user what mode they want.

```
find_window(keywords) → capture_screen(window_id) → read with vision
```

To extract precise text:
```
find_window → capture_screen(window_id) → apply_ocr()
```

To OCR specific regions:
```
find_window → capture_screen(window_id, detect=true) → apply_ocr(crop from detect)
```
