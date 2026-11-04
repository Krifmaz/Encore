package app.encore.android.ui.theme

import androidx.compose.material3.Typography
import dev.krifmaz.kit.compose.theme.KitTypography

/*
 * The web player's type scale (web/css/app.css), from the kit's tokens (design/tokens.json):
 *   body      14px / 1.5, system sans-serif
 *   h1        28px, weight 750, letter-spacing -.01em   (hero: 30px, detail head: 40px → 28px on phones)
 *   h2        19px, weight 700
 *   h3        15px, weight 650
 *   .small    12px      .tile .t 650      .tile .s 13px muted     .kicker 12px uppercase, .1em, 700
 *   .stat b   26px 750 -.02em             .np-meta h2 26px (21px on phones)
 */
val EncoreTypography: Typography = KitTypography
