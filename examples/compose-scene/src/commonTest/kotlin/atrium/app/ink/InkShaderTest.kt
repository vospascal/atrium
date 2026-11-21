package atrium.app.ink

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import org.jetbrains.skia.Bitmap
import org.jetbrains.skia.ColorAlphaType
import org.jetbrains.skia.ColorType
import org.jetbrains.skia.Image
import org.jetbrains.skia.ImageInfo
import org.jetbrains.skia.Paint
import org.jetbrains.skia.Rect
import org.jetbrains.skia.Surface
import org.jetbrains.skia.impl.use as useSkia

class InkShaderTest {
    @Test
    fun whiteInkKeepsItsPigmentAtHorizontalAndVerticalFolds() {
        for (horizontal in listOf(true, false)) {
            val pixels = ByteArray(16 * 16 * 4)
            for (y in 0 until 16) {
                for (x in 0 until 16) {
                    if ((if (horizontal) y else x) >= 8) {
                        val offset = (y * 16 + x) * 4
                        pixels.fill(-1, offset, offset + 4)
                    }
                }
            }
            val edge = if (horizontal) pixelAt(pixels, 128, 111) else pixelAt(pixels, 111, 128)
            assertTrue(edge ushr 24 in 1..254, "The fold should be visible but translucent")
            assertEquals(0xFFFFFF, edge and 0xFFFFFF, "The fold should stay white")
        }
    }

    @Test
    fun clearWaterStaysTransparent() {
        for (finish in InkFinish.entries) {
            assertEquals(0, pixelAt(ByteArray(16 * 16 * 4), 128, 128, finish))
        }
    }

    @Test
    fun smokeLightsTheFoldMoreThanItsBody() {
        val pixels = ByteArray(16 * 16 * 4)
        for (y in 0 until 16) {
            val start = (y * 16 + 8) * 4
            pixels.fill(60, start, start + 8 * 4)
        }
        val fold = pixelAt(pixels, 128, 128, InkFinish.Smoke)
        val body = pixelAt(pixels, 208, 128, InkFinish.Smoke)
        assertTrue(body ushr 24 > 0, "The body should still be visible")
        assertTrue(fold ushr 24 > body ushr 24, "The fold should glow more than the body")
        assertEquals(0, pixelAt(pixels, 32, 128, InkFinish.Smoke))
    }

    private fun pixelAt(pixels: ByteArray, x: Int, y: Int, finish: InkFinish = InkFinish.Dye): Int {
        val info = ImageInfo(16, 16, ColorType.RGBA_8888, ColorAlphaType.PREMUL)
        val shader = Image.makeRaster(info, pixels, 16 * 4).useSkia { image ->
            makeInkShader(image, dyeCellPixels = 16f, detailScale = 256f / 2.5f, seconds = 0f, finish = finish)
        }
        return shader.useSkia {
            Surface.makeRasterN32Premul(256, 256).useSkia { surface ->
                Paint().useSkia { paint ->
                    paint.shader = shader
                    surface.canvas.clear(0)
                    surface.canvas.drawRect(Rect.makeWH(256f, 256f), paint)
                }
                Bitmap().useSkia { bitmap ->
                    bitmap.allocN32Pixels(256, 256)
                    assertTrue(surface.readPixels(bitmap, 0, 0))
                    bitmap.getColor(x, y)
                }
            }
        }
    }
}
