package app.encore.android.ui.theme

import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import app.encore.android.data.session.ThemeChoice

// The logo's colours (web/css/app.css :root[data-theme="encore"]): charcoal and cream.
object EncoreColors {
    val Background = Color(0xFF161617)
    val Surface = Color(0xFF1C1C1E)
    val SurfaceContainer = Color(0xFF222224)
    val SurfaceContainerHigh = Color(0xFF2A2A2D)
    val Outline = Color(0xFF353538)
    val Cream = Color(0xFFF0EAD6)
    val CreamDeep = Color(0xFFD6C49A)
    val Muted = Color(0xFFAAA493)
    val Faint = Color(0xFF77725F)

    /** The accent: cream into gold, as on the logo. */
    val AccentGradient: Brush = Brush.linearGradient(listOf(Cream, CreamDeep))
}

private val EncoreScheme: ColorScheme = darkColorScheme(
    primary = EncoreColors.Cream,
    onPrimary = EncoreColors.Background,
    primaryContainer = EncoreColors.SurfaceContainerHigh,
    onPrimaryContainer = EncoreColors.Cream,
    secondary = EncoreColors.CreamDeep,
    onSecondary = EncoreColors.Background,
    secondaryContainer = EncoreColors.SurfaceContainerHigh,
    onSecondaryContainer = EncoreColors.Cream,
    tertiary = EncoreColors.CreamDeep,
    onTertiary = EncoreColors.Background,
    background = EncoreColors.Background,
    onBackground = EncoreColors.Cream,
    surface = EncoreColors.Surface,
    onSurface = EncoreColors.Cream,
    surfaceVariant = EncoreColors.SurfaceContainer,
    onSurfaceVariant = EncoreColors.Muted,
    surfaceContainerLowest = EncoreColors.Background,
    surfaceContainerLow = EncoreColors.Surface,
    surfaceContainer = EncoreColors.SurfaceContainer,
    surfaceContainerHigh = EncoreColors.SurfaceContainerHigh,
    surfaceContainerHighest = EncoreColors.Outline,
    outline = EncoreColors.Outline,
    outlineVariant = EncoreColors.Faint,
)

// web/css/app.css :root[data-theme="oled"]: true black, so pixels switch off.
private val OledScheme: ColorScheme = darkColorScheme(
    primary = EncoreColors.Cream,
    onPrimary = Color.Black,
    primaryContainer = Color(0xFF17171B),
    onPrimaryContainer = EncoreColors.Cream,
    secondary = EncoreColors.CreamDeep,
    onSecondary = Color.Black,
    secondaryContainer = Color(0xFF17171B),
    onSecondaryContainer = EncoreColors.Cream,
    tertiary = EncoreColors.CreamDeep,
    onTertiary = Color.Black,
    background = Color.Black,
    onBackground = EncoreColors.Cream,
    surface = Color.Black,
    onSurface = EncoreColors.Cream,
    surfaceVariant = Color(0xFF0B0B0D),
    onSurfaceVariant = Color(0xFF9A9AA8),
    surfaceContainerLowest = Color.Black,
    surfaceContainerLow = Color.Black,
    surfaceContainer = Color(0xFF0B0B0D),
    surfaceContainerHigh = Color(0xFF17171B),
    surfaceContainerHighest = Color(0xFF232329),
    outline = Color(0xFF232329),
    outlineVariant = Color(0xFF66666F),
)

// web/css/app.css :root[data-theme="light"]; the accent turns charcoal so it reads on white.
private val LightScheme: ColorScheme = lightColorScheme(
    primary = Color(0xFF2A2A2D),
    onPrimary = EncoreColors.Cream,
    primaryContainer = Color(0xFFF0F0F7),
    onPrimaryContainer = Color(0xFF16161F),
    secondary = Color(0xFF8A7A4E),
    onSecondary = Color.White,
    secondaryContainer = Color(0xFFF0F0F7),
    onSecondaryContainer = Color(0xFF16161F),
    tertiary = Color(0xFF8A7A4E),
    onTertiary = Color.White,
    background = Color(0xFFF6F6FB),
    onBackground = Color(0xFF16161F),
    surface = Color(0xFFFFFFFF),
    onSurface = Color(0xFF16161F),
    surfaceVariant = Color(0xFFF0F0F7),
    onSurfaceVariant = Color(0xFF5D5D75),
    surfaceContainerLowest = Color(0xFFFFFFFF),
    surfaceContainerLow = Color(0xFFFAFAFD),
    surfaceContainer = Color(0xFFF0F0F7),
    surfaceContainerHigh = Color(0xFFE9E9F2),
    surfaceContainerHighest = Color(0xFFE2E2EE),
    outline = Color(0xFFE2E2EE),
    outlineVariant = Color(0xFF8D8DA5),
)

fun isDarkTheme(theme: ThemeChoice): Boolean = theme != ThemeChoice.LIGHT

@Composable
fun EncoreTheme(theme: ThemeChoice, content: @Composable () -> Unit) {
    val scheme = when (theme) {
        ThemeChoice.ENCORE -> EncoreScheme
        ThemeChoice.OLED -> OledScheme
        ThemeChoice.LIGHT -> LightScheme
    }
    MaterialTheme(colorScheme = scheme, content = content)
}
