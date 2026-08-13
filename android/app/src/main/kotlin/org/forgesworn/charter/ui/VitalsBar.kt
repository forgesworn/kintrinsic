package org.forgesworn.charter.ui

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.view.Gravity
import android.view.View
import android.widget.LinearLayout
import android.widget.TextView

/**
 * The shade's vital-signs strip: charge on the left, reach on the right,
 * drawn rather than glyphed so they read at a glance from across a room and
 * scale with the shade instead of with a font.
 *
 * Deliberately gauges, not numbers alone: the ward this is for may not read
 * well (or at all) yet. A filling bar and a fan of bars are legible to a
 * five-year-old; "43%" and "LTE" are not.
 */
class VitalsBar(context: Context) : LinearLayout(context) {

    private val battery = BatteryGauge(context)
    private val batteryText = chipText(context)
    private val wifiGauge = WifiFan(context)
    private val wifiText = chipText(context)
    private val signalGauge = SignalBars(context)
    private val signalText = chipText(context)
    private val offlineText = chipText(context).apply {
        text = "No signal"
        setTextColor(Color.parseColor(Vitals.COLOR_LOW))
    }

    init {
        orientation = HORIZONTAL
        gravity = Gravity.CENTER_VERTICAL

        addView(battery)
        addView(batteryText, gap(6))

        // Push the reach cluster to the far edge.
        addView(View(context), LayoutParams(0, 1).apply { weight = 1f })

        addView(offlineText)
        addView(wifiGauge, gap(0))
        addView(wifiText, gap(6))
        addView(signalGauge, gap(14))
        addView(signalText, gap(6))
    }

    fun update(s: Vitals.Snapshot) {
        battery.set(s.batteryPercent, s.charging)
        batteryText.text = Vitals.batteryText(s.batteryPercent)
        batteryText.setTextColor(Color.parseColor(Vitals.batteryColor(s.batteryPercent)))

        wifiGauge.visibility = if (s.wifi) VISIBLE else GONE
        wifiText.visibility = if (s.wifi) VISIBLE else GONE
        wifiGauge.set(s.wifiLevel)
        wifiText.text = "Wi-Fi"

        // The chip shows whenever there is something true to say about the
        // radio — in service, or emergency-only, or "airplane mode" (a cause
        // the ward can act on). Bars only when the radio is actually carrying.
        val chip = s.mobileLabel.isNotEmpty()
        signalGauge.visibility = if (chip && s.mobile) VISIBLE else GONE
        signalText.visibility = if (chip) VISIBLE else GONE
        signalGauge.set(s.mobileLevel)
        signalText.text = s.mobileLabel
        signalText.setTextColor(
            Color.parseColor(
                // Anything other than a carrying radio is a caution, not chrome.
                if (s.mobile) Vitals.COLOR_TEXT else Vitals.COLOR_LOW,
            ),
        )

        // Only fall back to the bare "No signal" when the chip has nothing more
        // specific to say — "Airplane mode / No signal" side by side is noise,
        // and the named cause is always the more useful of the two.
        offlineText.visibility = if (s.offline && !chip) VISIBLE else GONE
    }

    private fun chipText(context: Context) = TextView(context).apply {
        textSize = 15f
        setTextColor(Color.parseColor(Vitals.COLOR_TEXT))
    }

    private fun gap(startDp: Int) = LayoutParams(
        LayoutParams.WRAP_CONTENT,
        LayoutParams.WRAP_CONTENT,
    ).apply { marginStart = (startDp * resources.displayMetrics.density).toInt() }
}

/** Shared scaling + paint for the drawn gauges. */
private abstract class Gauge(context: Context, val wDp: Int, val hDp: Int) : View(context) {
    protected val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    protected fun dp(v: Float) = v * resources.displayMetrics.density
    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        setMeasuredDimension(dp(wDp.toFloat()).toInt(), dp(hDp.toFloat()).toInt())
    }
}

/**
 * A battery that fills and changes colour, with a charging bolt. The outline
 * stays visible at 0% so an empty battery still looks like a battery and not
 * like a rendering failure.
 */
private class BatteryGauge(context: Context) : Gauge(context, 34, 17) {
    private var pct: Int? = null
    private var charging = false

    fun set(pct: Int?, charging: Boolean) {
        this.pct = pct
        this.charging = charging
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        val stroke = dp(1.5f)
        val nub = dp(3f)
        val body = RectF(
            stroke / 2f,
            stroke / 2f,
            width - nub - stroke / 2f,
            height - stroke / 2f,
        )
        val radius = dp(3f)
        val tint = Color.parseColor(Vitals.batteryColor(pct))

        paint.style = Paint.Style.STROKE
        paint.strokeWidth = stroke
        paint.color = Color.parseColor(Vitals.COLOR_MUTED)
        canvas.drawRoundRect(body, radius, radius, paint)

        // The terminal nub, so the shape reads as a battery instantly.
        paint.style = Paint.Style.FILL
        val nubHeight = height * 0.42f
        canvas.drawRoundRect(
            RectF(
                width - nub,
                (height - nubHeight) / 2f,
                width.toFloat(),
                (height + nubHeight) / 2f,
            ),
            dp(1f), dp(1f), paint,
        )

        val level = pct ?: return // unknown: outline only, never a guessed fill
        val inset = stroke + dp(1.5f)
        val fillable = RectF(
            body.left + inset,
            body.top + inset,
            body.right - inset,
            body.bottom - inset,
        )
        paint.color = tint
        if (level > 0) {
            canvas.drawRoundRect(
                RectF(
                    fillable.left,
                    fillable.top,
                    fillable.left + fillable.width() * (level / 100f),
                    fillable.bottom,
                ),
                dp(1.5f), dp(1.5f), paint,
            )
        }

        if (charging) {
            // A bolt struck through the fill — visible against both grounds.
            val cx = body.centerX()
            val cy = body.centerY()
            val h = body.height() * 0.34f
            val w = body.width() * 0.11f
            val bolt = Path().apply {
                moveTo(cx + w, cy - h)
                lineTo(cx - w * 1.1f, cy + h * 0.15f)
                lineTo(cx + w * 0.15f, cy + h * 0.15f)
                lineTo(cx - w, cy + h)
                lineTo(cx + w * 1.1f, cy - h * 0.15f)
                lineTo(cx - w * 0.15f, cy - h * 0.15f)
                close()
            }
            paint.color = Color.parseColor("#0B1021")
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(2f)
            canvas.drawPath(bolt, paint)
            paint.style = Paint.Style.FILL
            paint.color = Color.WHITE
            canvas.drawPath(bolt, paint)
        }
    }
}

/** Four ascending bars; lit up to the level, dim beyond, all dim if unknown. */
private class SignalBars(context: Context) : Gauge(context, 22, 17) {
    private var level: Int? = null

    fun set(level: Int?) {
        this.level = level
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        paint.style = Paint.Style.FILL
        val gap = dp(2f)
        val barWidth = (width - gap * (Vitals.BARS - 1)) / Vitals.BARS
        val lit = level ?: 0
        for (i in 0 until Vitals.BARS) {
            val tall = height * (0.4f + 0.2f * i)
            val left = i * (barWidth + gap)
            paint.color = Color.parseColor(
                if (level != null && i < lit) Vitals.COLOR_OK else Vitals.COLOR_DIM,
            )
            canvas.drawRoundRect(
                RectF(left, height - tall, left + barWidth, height.toFloat()),
                dp(1f), dp(1f), paint,
            )
        }
    }
}

/** The Wi-Fi fan: a dot and three arcs, lit the same way as the bars. */
private class WifiFan(context: Context) : Gauge(context, 22, 17) {
    private var level: Int? = null

    fun set(level: Int?) {
        this.level = level
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        val cx = width / 2f
        val cy = height - dp(1.5f)
        val lit = level ?: 0
        fun tint(index: Int) = Color.parseColor(
            if (level != null && index < lit) Vitals.COLOR_OK else Vitals.COLOR_DIM,
        )

        paint.style = Paint.Style.FILL
        paint.color = tint(0)
        canvas.drawCircle(cx, cy - dp(1f), dp(1.8f), paint)

        paint.style = Paint.Style.STROKE
        paint.strokeWidth = dp(2f)
        paint.strokeCap = Paint.Cap.ROUND
        for (arc in 1..3) {
            val r = dp(3.2f + 3.1f * arc)
            paint.color = tint(arc)
            canvas.drawArc(RectF(cx - r, cy - r, cx + r, cy + r), 225f, 90f, false, paint)
        }
    }
}
