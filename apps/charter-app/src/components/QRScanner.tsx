import { useEffect, useRef, useCallback, useState } from "react";
import jsQR from "jsqr";

/**
 * Live QR scanner: native getUserMedia + jsQR frame decoding, ported from
 * signet-app's proven component (the "works quite well" one). Differences:
 * - `onScan` returns a boolean — `false` means "that QR wasn't ours", and the
 *   camera KEEPS scanning instead of freezing on a stray code.
 * - Styled with Kintrinsic tokens.
 *
 * Zoom is progressive enhancement: cameras that expose a hardware `zoom`
 * capability get optical zoom (most Android); everyone else gets a digital
 * center-crop zoom so a small laptop-screen QR still fills the frame.
 */

interface Props {
  onScan: (data: string) => boolean;
  active: boolean;
}

export const PREFERRED_ZOOM = 1.6;
const ZOOM_BUTTON_STEP = 0.2;
const DIGITAL_ZOOM_RANGE = { min: 1, max: 3.5, step: 0.1 };
const VIDEO_CONSTRAINTS: MediaTrackConstraints = {
  facingMode: { ideal: "environment" },
  width: { ideal: 1920 },
  height: { ideal: 1080 },
  frameRate: { ideal: 30, max: 30 },
};

export function scanSourceRect(videoWidth: number, videoHeight: number, zoom: number) {
  const safeWidth = Math.max(0, videoWidth);
  const safeHeight = Math.max(0, videoHeight);
  const safeZoom = Number.isFinite(zoom) ? Math.max(1, Math.min(zoom, 6)) : 1;
  const width = safeWidth / safeZoom;
  const height = safeHeight / safeZoom;
  return {
    sx: (safeWidth - width) / 2,
    sy: (safeHeight - height) / 2,
    sw: width,
    sh: height,
  };
}

export function initialZoom(range: { min: number; max: number } | null, preferred = PREFERRED_ZOOM) {
  if (!range) return Math.max(1, preferred);
  return Math.max(range.min, Math.min(preferred, range.max));
}

export function nextZoom(
  current: number,
  range: { min: number; max: number; step?: number },
  direction: -1 | 1,
  buttonStep = ZOOM_BUTTON_STEP,
) {
  const step = Math.max(range.step ?? buttonStep, buttonStep);
  const bounded = Math.max(range.min, Math.min(range.max, current + direction * step));
  return Number(bounded.toFixed(2));
}

export function QRScanner({ onScan, active }: Props) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const trackRef = useRef<MediaStreamTrack | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const doneRef = useRef(false);
  const onScanRef = useRef(onScan);
  const [error, setError] = useState<string | null>(null);
  const [zoomRange, setZoomRange] = useState<{ min: number; max: number; step: number } | null>(null);
  const [zoom, setZoom] = useState(1);
  const [hardwareZoom, setHardwareZoom] = useState(false);
  const zoomRef = useRef(1);
  const hardwareZoomRef = useRef(false);
  zoomRef.current = zoom;
  hardwareZoomRef.current = hardwareZoom;
  onScanRef.current = onScan;

  const stopCamera = useCallback(() => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    if (streamRef.current) {
      streamRef.current.getTracks().forEach((t) => t.stop());
      streamRef.current = null;
    }
    trackRef.current = null;
    if (videoRef.current) {
      videoRef.current.srcObject = null;
    }
  }, []);

  const applyZoom = useCallback(
    (value: number) => {
      const range = zoomRange ?? DIGITAL_ZOOM_RANGE;
      const bounded = Number(Math.max(range.min, Math.min(range.max, value)).toFixed(2));
      zoomRef.current = bounded;
      setZoom(bounded);
      if (!hardwareZoomRef.current) return;
      const track = trackRef.current as
        | (MediaStreamTrack & { applyConstraints?: (c: unknown) => Promise<void> })
        | null;
      if (!track?.applyConstraints) return;
      try {
        void track.applyConstraints({ advanced: [{ zoom: bounded }] });
      } catch {
        // Camera rejected the constraint mid-stream — leave zoom where it was.
      }
    },
    [zoomRange],
  );

  const stepZoom = useCallback(
    (direction: -1 | 1) => {
      if (!zoomRange) return;
      applyZoom(nextZoom(zoomRef.current, zoomRange, direction));
    },
    [applyZoom, zoomRange],
  );

  const nudgeFocus = useCallback(() => {
    const track = trackRef.current as
      | (MediaStreamTrack & { applyConstraints?: (c: unknown) => Promise<void> })
      | null;
    if (!track?.applyConstraints) return;
    void track
      .applyConstraints({ advanced: [{ focusMode: "continuous" }, { exposureMode: "continuous" }] })
      .catch(() => {
        /* unsupported on many mobile browsers */
      });
  }, []);

  useEffect(() => {
    if (!active) {
      stopCamera();
      doneRef.current = false;
      setError(null);
      setZoomRange(null);
      setZoom(1);
      zoomRef.current = 1;
      setHardwareZoom(false);
      hardwareZoomRef.current = false;
      return;
    }

    let mounted = true;
    doneRef.current = false;

    async function start() {
      try {
        if (!navigator.mediaDevices?.getUserMedia) {
          throw new Error("no mediaDevices");
        }
        const stream = await navigator.mediaDevices.getUserMedia({
          video: VIDEO_CONSTRAINTS,
          audio: false,
        });

        if (!mounted) {
          stream.getTracks().forEach((t) => t.stop());
          return;
        }
        streamRef.current = stream;

        const track = stream.getVideoTracks()[0] ?? null;
        trackRef.current = track;
        const caps = track?.getCapabilities?.() as Record<string, unknown> | undefined;
        const zoomCap = caps?.zoom as { min?: number; max?: number; step?: number } | undefined;
        if (zoomCap && typeof zoomCap.max === "number" && zoomCap.max > (zoomCap.min ?? 1)) {
          const min = zoomCap.min ?? 1;
          const max = zoomCap.max;
          setHardwareZoom(true);
          hardwareZoomRef.current = true;
          setZoomRange({ min, max, step: zoomCap.step || 0.1 });
          const start = initialZoom({ min, max });
          zoomRef.current = start;
          setZoom(start);
          void (track as MediaStreamTrack & { applyConstraints?: (c: unknown) => Promise<void> })
            .applyConstraints?.({ advanced: [{ zoom: start }] })
            .catch(() => {
              /* optical zoom is best-effort */
            });
        } else {
          setHardwareZoom(false);
          hardwareZoomRef.current = false;
          setZoomRange(DIGITAL_ZOOM_RANGE);
          const start = initialZoom(DIGITAL_ZOOM_RANGE);
          zoomRef.current = start;
          setZoom(start);
        }

        const video = videoRef.current;
        if (!video) return;
        video.srcObject = stream;
        await video.play();

        scanFrame();
      } catch {
        if (mounted) setError("Couldn't open the camera. Check the browser's camera permission, or type the code below instead.");
      }
    }

    function scanFrame() {
      if (!mounted || doneRef.current) return;

      const video = videoRef.current;
      const canvas = canvasRef.current;
      if (!video || !canvas || video.readyState < video.HAVE_ENOUGH_DATA) {
        timerRef.current = setTimeout(scanFrame, 250);
        return;
      }

      canvas.width = video.videoWidth;
      canvas.height = video.videoHeight;
      const ctx = canvas.getContext("2d", { willReadFrequently: true });
      if (!ctx) {
        timerRef.current = setTimeout(scanFrame, 250);
        return;
      }

      const digitalZoom = hardwareZoomRef.current ? 1 : zoomRef.current;
      const source = scanSourceRect(video.videoWidth, video.videoHeight, digitalZoom);
      ctx.drawImage(video, source.sx, source.sy, source.sw, source.sh, 0, 0, canvas.width, canvas.height);
      const imageData = ctx.getImageData(0, 0, canvas.width, canvas.height);
      const result = jsQR(imageData.data, imageData.width, imageData.height, {
        inversionAttempts: "attemptBoth",
      });

      if (result?.data && !doneRef.current) {
        // Only stop on a code the caller accepts — a stray QR keeps scanning.
        if (onScanRef.current(result.data)) {
          doneRef.current = true;
          stopCamera();
          return;
        }
      }

      // Scan several times per second so small laptop-screen QRs lock quickly.
      timerRef.current = setTimeout(scanFrame, 150);
    }

    start();
    return () => {
      mounted = false;
      stopCamera();
    };
  }, [active, stopCamera]);

  const digitalPreviewZoom = hardwareZoom ? 1 : zoom;

  return (
    <div
      style={{
        position: "relative",
        width: "100%",
        borderRadius: "var(--radius-card)",
        overflow: "hidden",
        background: "#05070b",
        height: "min(58vh, 440px)",
        minHeight: 320,
      }}
    >
      {error && (
        <div style={{ padding: 16, textAlign: "center", color: "#fff", fontSize: "0.92rem" }}>{error}</div>
      )}
      <video
        ref={videoRef}
        onClick={nudgeFocus}
        style={{
          width: "100%",
          height: "100%",
          minHeight: 320,
          display: error ? "none" : "block",
          objectFit: "cover",
          transform: digitalPreviewZoom > 1 ? `scale(${digitalPreviewZoom})` : undefined,
          transformOrigin: "center",
          cursor: active && !error ? "crosshair" : undefined,
        }}
        playsInline
        muted
      />
      {active && !error && (
        <div
          style={{
            position: "absolute",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            width: "min(74vw, 300px, calc(100% - 48px))",
            aspectRatio: "1 / 1",
            border: "2px solid rgba(255,255,255,0.7)",
            borderRadius: 12,
            boxShadow: "0 0 0 999px rgba(0,0,0,0.18)",
            pointerEvents: "none",
          }}
        />
      )}
      {active && !error && zoomRange && (
        <div
          style={{
            position: "absolute",
            bottom: 12,
            left: 12,
            right: 12,
            display: "flex",
            alignItems: "center",
            gap: 8,
            padding: "10px 12px",
            background: "rgba(0,0,0,0.62)",
            borderRadius: 8,
          }}
        >
          <button
            type="button"
            onClick={() => stepZoom(-1)}
            aria-label="Zoom out"
            disabled={zoom <= zoomRange.min}
            style={{
              width: 40,
              height: 40,
              border: 0,
              borderRadius: 8,
              background: "rgba(255,255,255,0.18)",
              color: "#fff",
              fontSize: 22,
              fontWeight: 700,
              lineHeight: 1,
              opacity: zoom <= zoomRange.min ? 0.45 : 1,
            }}
          >
            -
          </button>
          <input
            type="range"
            min={zoomRange.min}
            max={zoomRange.max}
            step={zoomRange.step}
            value={zoom}
            onChange={(e) => applyZoom(Number(e.target.value))}
            style={{ flex: 1, accentColor: "var(--brand)", minWidth: 0 }}
            aria-label="Camera zoom"
          />
          <button
            type="button"
            onClick={() => stepZoom(1)}
            aria-label="Zoom in"
            disabled={zoom >= zoomRange.max}
            style={{
              width: 40,
              height: 40,
              border: 0,
              borderRadius: 8,
              background: "rgba(255,255,255,0.18)",
              color: "#fff",
              fontSize: 22,
              fontWeight: 700,
              lineHeight: 1,
              opacity: zoom >= zoomRange.max ? 0.45 : 1,
            }}
          >
            +
          </button>
          <span
            style={{
              color: "#fff",
              fontSize: 12,
              fontVariantNumeric: "tabular-nums",
              minWidth: 34,
              textAlign: "right",
            }}
          >
            {zoom.toFixed(1)}x
          </span>
        </div>
      )}
      <canvas ref={canvasRef} style={{ display: "none" }} />
    </div>
  );
}
