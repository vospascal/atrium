package atrium.app

import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import org.jetbrains.skia.ColorAlphaType
import org.jetbrains.skia.ColorType
import org.jetbrains.skia.FilterMipmap
import org.jetbrains.skia.FilterMode
import org.jetbrains.skia.FilterTileMode
import org.jetbrains.skia.Image
import org.jetbrains.skia.ImageInfo
import org.jetbrains.skia.MipmapMode
import org.jetbrains.skia.RuntimeEffect
import org.jetbrains.skia.RuntimeShaderBuilder
import org.jetbrains.skia.Shader
import org.jetbrains.skia.impl.use as useSkia

/** Seconds after which the detail's clock wraps, keeping noise coordinates precise. */
private const val ClockWrap = 3600f

/**
 * Draws simulated ink as wet ink rather than smoke: the [InkFluid] dye grid,
 * smoothly upscaled. Thin veils stay nearly clear, dense ink deepens in hue
 * (pigment absorbs more where light passes through more of it), and where a
 * sheet of ink folds edge-on its coverage changes sharply and a dark line
 * shows. A slowly drifting nudge and a soft grain add texture below a cell.
 * Motion and structure are the simulation's.
 *
 * The result is premultiplied and transparent outside the ink, so the same
 * paint sits on any backdrop.
 */
private const val InkSksl = """
uniform shader dye;   // one premultiplied pixel per dye cell
uniform float cell;   // pixels per dye cell
uniform float scale;  // pixels per unit of detail
uniform float time;

float hash(float2 p) {
    p = fract(p * float2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

float noise(float2 p) {
    float2 i = floor(p);
    float2 f = fract(p);
    float2 s = f * f * (3.0 - 2.0 * f);
    float a = hash(i);
    float b = hash(i + float2(1.0, 0.0));
    float c = hash(i + float2(0.0, 1.0));
    float d = hash(i + float2(1.0, 1.0));
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y);
}

float fbm(float2 p) {
    float sum = 0.0;
    float amplitude = 0.5;
    for (int octave = 0; octave < 4; octave++) {
        sum += amplitude * noise(p);
        p = p * 2.03 + float2(17.1, 9.2);
        amplitude *= 0.5;
    }
    return sum;
}

half4 main(float2 fragCoord) {
    float2 p = fragCoord / scale;
    float2 q = fragCoord / cell;
    float2 nudge = float2(fbm(p * 3.0 + float2(0.0, time * 0.05)), fbm(p * 3.0 + float2(4.1, 2.3) - time * 0.04)) - 0.5;
    q += nudge * 0.6;
    half4 sampled = dye.eval(q);
    float amount = sampled.a;

    // Folds: where a sheet of ink turns edge-on its coverage changes
    // sharply, and wet ink shows a dark line there.
    // Sampled 1.6 cells apart, so the line follows the fold, not the grid.
    half4 right = dye.eval(q + float2(1.6, 0.0));
    half4 left = dye.eval(q - float2(1.6, 0.0));
    half4 down = dye.eval(q + float2(0.0, 1.6));
    half4 up = dye.eval(q - float2(0.0, 1.6));
    float edge = length(float2(right.a - left.a, down.a - up.a)) * 0.7;
    float fold = smoothstep(0.04, 0.3, edge);
    if (amount < 0.004 && fold < 0.01) { return half4(0.0); }

    // Outside a fold, borrow the pigment from the most covered neighbor.
    half4 pigment = sampled;
    if (amount <= 0.004) {
        half4 horizontal = right.a > left.a ? right : left;
        half4 vertical = down.a > up.a ? down : up;
        pigment = horizontal.a > vertical.a ? horizontal : vertical;
    }
    float3 color = float3(pigment.rgb) / max(float(pigment.a), 0.004);

    // Thin veils stay clear; dense ink and folds go deeper in hue, as
    // pigment does where light passes through more of it.
    float grain = fbm(p * 5.0 + nudge * 2.0);
    float body = pow(amount, 1.25) * (1.05 + 0.4 * grain);
    float alpha = clamp(max(body * 0.95, fold * 0.55), 0.0, 0.95);
    float depth = 1.0 + 1.6 * amount + 1.4 * fold;
    color = pow(clamp(color, 0.0, 1.0), float3(depth));
    return half4(half3(color * alpha), half(alpha));
}
"""

private val InkEffect: RuntimeEffect by lazy { RuntimeEffect.makeForShader(InkSksl) }

private val Smooth = FilterMipmap(FilterMode.LINEAR, MipmapMode.NONE)

internal enum class InkFinish { Dye, Smoke }

internal fun makeInkShader(
    image: Image,
    dyeCellPixels: Float,
    detailScale: Float,
    seconds: Float,
    finish: InkFinish = InkFinish.Dye,
): Shader {
    val effect = when (finish) {
        InkFinish.Dye -> InkEffect
        InkFinish.Smoke -> SmokeEffect
    }
    return RuntimeShaderBuilder(effect).useSkia { builder ->
        image.makeShader(FilterTileMode.CLAMP, FilterTileMode.CLAMP, Smooth, null).useSkia { dye ->
            builder.child("dye", dye)
            builder.uniform("cell", dyeCellPixels)
            builder.uniform("scale", detailScale)
            builder.uniform("time", seconds % ClockWrap)
            builder.makeShader()
        }
    }
}

/**
 * Paints [fluid]'s ink over this canvas, one water cell per [cell] pixels,
 * [seconds] into its detail. [tint], when set, replaces every ink's color.
 */
internal fun DrawScope.drawInk(
    fluid: InkFluid,
    pixels: ByteArray,
    cell: Float,
    seconds: Float,
    tint: Color? = null,
    opacity: Float = 1f,
    finish: InkFinish = InkFinish.Dye,
) {
    fluid.writePixels(pixels, tint)
    val info = ImageInfo(fluid.dyeColumns, fluid.dyeRows, ColorType.RGBA_8888, ColorAlphaType.PREMUL)
    Image.makeRaster(info, pixels, fluid.dyeColumns * 4).useSkia { image ->
        makeInkShader(
            image, cell / fluid.detail, maxOf(size.width, size.height) / 2.5f, seconds,
            finish = finish,
        ).useSkia { shader ->
            drawRect(brushOf(shader), alpha = opacity)
        }
    }
}

/**
 * A Skia shader as a Compose brush. Compose wraps Skia shaders on every
 * target this app builds, but only platform code can see the wrapper.
 */
internal expect fun brushOf(shader: Shader): Brush
