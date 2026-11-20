package atrium.app

import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.ShaderBrush
import androidx.compose.ui.graphics.asComposeShader
import org.jetbrains.skia.Shader

internal actual fun brushOf(shader: Shader): Brush = ShaderBrush(shader.asComposeShader())
