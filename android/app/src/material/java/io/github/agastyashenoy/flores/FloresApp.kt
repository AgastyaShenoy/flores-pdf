package io.github.agastyashenoy.flores

import android.app.Application
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.appcompat.app.AppCompatDelegate
import com.google.android.material.color.DynamicColors

class FloresApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // the wallpaper's colors on Android 12 and later
        DynamicColors.applyToActivitiesIfAvailable(this)
        applyTheme(Recents(this).theme)
    }

    companion object {
        /** Light or dark for [theme] (0 system, 1 light, 2 dark, 3 black); black is dark with
         *  black surfaces, which [FloresActivity] adds. */
        fun applyTheme(theme: Int) = AppCompatDelegate.setDefaultNightMode(
            when (theme) {
                1 -> AppCompatDelegate.MODE_NIGHT_NO
                2, 3 -> AppCompatDelegate.MODE_NIGHT_YES
                else -> AppCompatDelegate.MODE_NIGHT_FOLLOW_SYSTEM
            }
        )
    }
}

/** The app's activities: drawn edge to edge, in the theme from the settings. */
open class FloresActivity : AppCompatActivity() {
    private var themeShown = -1

    override fun onCreate(savedInstanceState: Bundle?) {
        themeShown = Recents(this).theme
        if (themeShown == 3) theme.applyStyle(R.style.ThemeOverlay_Flores_Black, true)
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
    }

    override fun onResume() {
        super.onResume()
        // Dark and Black are both night mode, so switching between them needs a new start
        if (Recents(this).theme != themeShown) recreate()
    }
}
