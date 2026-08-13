package org.forgesworn.charter.ui

import android.content.Context
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.util.TypedValue
import android.view.Gravity
import android.view.ViewGroup
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Kintrinsic's look, in one place — the Android half of the design system whose
 * other half is `apps/charter-app/src/theme.css`. The two must be read
 * together: same wax-seal brand, same radii, same type scale, same minimum
 * tap target. Change a value here and change its twin there.
 *
 * Before this existed, every screen invented its own hex codes and paddings
 * (#0B1021 navy, #7C89B8 slate, #2C3A6E slabs — none of them Kintrinsic's
 * colours), so a parent's phone and a child's phone looked like two different
 * products. A family should be able to tell they are holding the same thing.
 *
 * Two moods, one language:
 *  - PAPER, for the ward's app: the warm off-white Kintrinsic and the website
 *    already use.
 *  - INK, for the lock shade: Kintrinsic's own warm dark. Dark is right for a
 *    screen a child meets at bedtime — a full-page white shade is a torch in
 *    a dark room, and costs more battery on OLED — but it is a warm dark
 *    derived from the paper, not an unrelated blue.
 */
object CharterTheme {

    // ── Paper: the ward's app (mirrors --bg / --surface / --text …) ────────
    const val PAPER_BG = "#F7F6F2"
    const val PAPER_SURFACE = "#FFFFFF"
    const val PAPER_TEXT = "#1C1C1E"
    const val PAPER_TEXT_2 = "#6B6B70"
    const val PAPER_LINE = "#ECEAE3"

    // ── Ink: the lock shade ───────────────────────────────────────────────
    const val INK_BG = "#16130F"
    const val INK_SURFACE = "#211D18"
    const val INK_TEXT = "#F7F6F2"
    const val INK_TEXT_2 = "#A9A296"
    const val INK_LINE = "#332C24"

    // ── Brand + status, shared by both moods (mirrors --brand / --ok / …) ──
    /** The wax seal. Used ONCE per screen, on the way forward. */
    const val BRAND = "#8F2A24"
    const val BRAND_PRESS = "#732019"
    const val OK = "#2E7D5B"
    /** The same green lifted to clear 4.5:1 on ink. */
    const val OK_ON_INK = "#7CD9A6"
    const val WARN = "#C4860F"
    const val WARN_ON_INK = "#E0A93F"

    // ── Shape + rhythm (mirrors --radius-* / --tap) ────────────────────────
    const val RADIUS_CARD = 18
    const val RADIUS_CONTROL = 12
    /** Android's own floor is 48dp; the CSS says 44. Take the larger. */
    const val TAP = 48

    // ── Type scale (mirrors --fs-*) ───────────────────────────────────────
    const val FS_DISPLAY = 64f
    const val FS_TITLE = 28f
    const val FS_SECTION = 20f
    const val FS_BODY = 16f
    const val FS_SMALL = 14f

    fun dp(ctx: Context, v: Int): Int =
        TypedValue.applyDimension(
            TypedValue.COMPLEX_UNIT_DIP,
            v.toFloat(),
            ctx.resources.displayMetrics,
        ).toInt()

    fun color(hex: String): Int = Color.parseColor(hex)

    /** Body/label text. `top` is the space ABOVE it, in dp. */
    fun text(
        ctx: Context,
        s: String,
        size: Float = FS_BODY,
        color: String = PAPER_TEXT,
        top: Int = 0,
        centre: Boolean = false,
        bold: Boolean = false,
    ) = TextView(ctx).apply {
        text = s
        textSize = size
        setTextColor(color(color))
        if (bold) setTypeface(typeface, android.graphics.Typeface.BOLD)
        if (centre) gravity = Gravity.CENTER
        setPadding(0, dp(ctx, top), 0, 0)
        // Loosen the default leading — dense paragraphs on a small screen were
        // the other half of why these screens read as unfinished.
        setLineSpacing(dp(ctx, 3).toFloat(), 1f)
    }

    /**
     * A real button: rounded to the system's control radius, with a pressed
     * state, a proper tap target, and sentence case.
     *
     * Replaces two habits that made every screen look unfinished — the raw
     * platform `Button` (grey, ALL-CAPS, Material's own shape) and
     * `setBackgroundColor`, which REPLACES the background drawable outright
     * and so throws away the corners and the ripple, leaving a flat slab.
     */
    fun button(
        ctx: Context,
        label: String,
        fill: String,
        textColor: String,
        pressed: String = BRAND_PRESS,
    ) = Button(ctx).apply {
        text = label
        isAllCaps = false
        textSize = FS_BODY
        setTextColor(color(textColor))
        minHeight = dp(ctx, TAP)
        minimumHeight = dp(ctx, TAP)
        stateListAnimator = null
        elevation = 0f
        setPadding(dp(ctx, 20), dp(ctx, 12), dp(ctx, 20), dp(ctx, 12))
        background = RippleDrawable(
            ColorStateList.valueOf(color(pressed)),
            GradientDrawable().apply {
                shape = GradientDrawable.RECTANGLE
                cornerRadius = dp(ctx, RADIUS_CONTROL).toFloat()
                setColor(color(fill))
            },
            null,
        )
    }

    /** The one forward action on a screen — the wax seal. */
    fun primaryButton(ctx: Context, label: String) =
        button(ctx, label, BRAND, "#FFFFFF", BRAND_PRESS)

    /** Everything else: outlined, so the primary stays the loudest thing. */
    fun secondaryButton(ctx: Context, label: String, onInk: Boolean = false) =
        button(
            ctx,
            label,
            if (onInk) INK_SURFACE else PAPER_SURFACE,
            if (onInk) INK_TEXT else PAPER_TEXT,
            if (onInk) INK_LINE else PAPER_LINE,
        ).apply {
            background = RippleDrawable(
                ColorStateList.valueOf(color(if (onInk) INK_LINE else PAPER_LINE)),
                GradientDrawable().apply {
                    shape = GradientDrawable.RECTANGLE
                    cornerRadius = dp(ctx, RADIUS_CONTROL).toFloat()
                    setColor(color(if (onInk) INK_SURFACE else PAPER_SURFACE))
                    setStroke(dp(ctx, 1), color(if (onInk) INK_LINE else PAPER_LINE))
                },
                null,
            )
        }

    /** A card: the unit Kintrinsic groups everything into. */
    fun card(ctx: Context, onInk: Boolean = false) = LinearLayout(ctx).apply {
        orientation = LinearLayout.VERTICAL
        setPadding(dp(ctx, 18), dp(ctx, 16), dp(ctx, 18), dp(ctx, 16))
        background = GradientDrawable().apply {
            shape = GradientDrawable.RECTANGLE
            cornerRadius = dp(ctx, RADIUS_CARD).toFloat()
            setColor(color(if (onInk) INK_SURFACE else PAPER_SURFACE))
            setStroke(dp(ctx, 1), color(if (onInk) INK_LINE else PAPER_LINE))
        }
    }

    /** Vertical space between blocks, as a view (clearer than stray padding). */
    fun gap(ctx: Context, height: Int) = android.view.View(ctx).apply {
        layoutParams = LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            dp(ctx, height),
        )
    }

    /** Standard block margins for a stacked column. */
    fun stackParams(ctx: Context, top: Int = 0) =
        LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT,
        ).apply { topMargin = dp(ctx, top) }
}
