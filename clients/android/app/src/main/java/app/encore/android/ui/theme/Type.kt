package app.encore.android.ui.theme

import androidx.compose.material3.Typography
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/*
 * The web player's type scale (web/css/app.css):
 *   body      14px / 1.5, system sans-serif
 *   h1        28px, weight 750, letter-spacing -.01em   (hero: 30px, detail head: 40px → 28px on phones)
 *   h2        19px, weight 700
 *   h3        15px, weight 650
 *   .small    12px      .tile .t 650      .tile .s 13px muted     .kicker 12px uppercase, .1em, 700
 *   .stat b   26px 750 -.02em             .np-meta h2 26px (21px on phones)
 */
private val Sans = FontFamily.SansSerif

val EncoreTypography: Typography = Typography(
    // the biggest numbers / titles: stats, the full-screen player title
    displaySmall = TextStyle(fontFamily = Sans, fontSize = 26.sp, lineHeight = 30.sp, fontWeight = FontWeight.Bold, letterSpacing = (-0.5).sp),
    // h1
    headlineLarge = TextStyle(fontFamily = Sans, fontSize = 30.sp, lineHeight = 36.sp, fontWeight = FontWeight.Bold, letterSpacing = (-0.3).sp),
    headlineMedium = TextStyle(fontFamily = Sans, fontSize = 28.sp, lineHeight = 34.sp, fontWeight = FontWeight.Bold, letterSpacing = (-0.3).sp),
    headlineSmall = TextStyle(fontFamily = Sans, fontSize = 24.sp, lineHeight = 30.sp, fontWeight = FontWeight.Bold, letterSpacing = (-0.2).sp),
    // h2 / h3
    titleLarge = TextStyle(fontFamily = Sans, fontSize = 19.sp, lineHeight = 24.sp, fontWeight = FontWeight.Bold, letterSpacing = (-0.2).sp),
    titleMedium = TextStyle(fontFamily = Sans, fontSize = 15.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.sp),
    titleSmall = TextStyle(fontFamily = Sans, fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.sp),
    // body (14px / 1.5) and the muted/small variants
    bodyLarge = TextStyle(fontFamily = Sans, fontSize = 15.sp, lineHeight = 22.sp, fontWeight = FontWeight.Normal, letterSpacing = 0.sp),
    bodyMedium = TextStyle(fontFamily = Sans, fontSize = 14.sp, lineHeight = 21.sp, fontWeight = FontWeight.Normal, letterSpacing = 0.sp),
    bodySmall = TextStyle(fontFamily = Sans, fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.Normal, letterSpacing = 0.sp),
    // buttons, tabs, badges, the kicker
    labelLarge = TextStyle(fontFamily = Sans, fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.sp),
    labelMedium = TextStyle(fontFamily = Sans, fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.2.sp),
    labelSmall = TextStyle(fontFamily = Sans, fontSize = 11.sp, lineHeight = 14.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.5.sp),
)
