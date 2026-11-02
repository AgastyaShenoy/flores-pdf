package io.github.agastyashenoy.flores

import android.view.View
import androidx.core.view.WindowInsetsCompat
import com.google.android.material.color.MaterialColors

/** A color of the theme, such as `com.google.android.material.R.attr.colorSurfaceContainer`. */
fun View.themeColor(attr: Int): Int = MaterialColors.getColor(this, attr)

/** The system bars' and display cutout's sizes. */
fun WindowInsetsCompat.bars() = getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
