package io.github.agastyashenoy.flores

import android.content.Context
import android.text.InputType
import android.view.LayoutInflater
import android.view.View
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView
import androidx.core.widget.NestedScrollView
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.textfield.TextInputEditText
import com.google.android.material.textfield.TextInputLayout

/** The pieces sheets and dialogs are made of: a column with a title, rows, section headings, and a
 *  dialog that asks for a line of text. */
object Sheets {
    /** A scrolling column for a bottom sheet, with [title] at its top if given. */
    fun column(c: Context, title: String? = null): LinearLayout {
        val column = LinearLayout(c).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(0, 0, 0, c.dp(16f))
        }
        if (title != null) {
            column.addView(TextView(c).apply {
                text = title
                setTextAppearance(com.google.android.material.R.style.TextAppearance_Material3_TitleLarge)
                setPadding(c.dp(24f), c.dp(4f), c.dp(24f), c.dp(12f))
                maxLines = 2
            })
        }
        return column
    }

    /** [column] in a scroll view, for sheets that can be long. */
    fun scrolling(column: LinearLayout): View = NestedScrollView(column.context).apply { addView(column) }

    fun section(c: Context, title: String): TextView = TextView(c).apply {
        text = title
        setTextAppearance(R.style.TextAppearance_Flores_Section)
        setPadding(c.dp(24f), c.dp(16f), c.dp(24f), c.dp(8f))
    }

    /** A row with an icon, a title and, if given, a line under it. */
    fun row(c: Context, icon: Int, title: String, subtitle: String? = null, onClick: () -> Unit): View {
        val row = LayoutInflater.from(c).inflate(R.layout.item_row, null)
        val iconView = row.findViewById<ImageView>(R.id.icon)
        if (icon != 0) iconView.setImageResource(icon) else iconView.visibility = View.GONE
        row.findViewById<TextView>(R.id.title).text = title
        if (subtitle != null) {
            row.findViewById<TextView>(R.id.subtitle).apply {
                text = subtitle
                visibility = View.VISIBLE
            }
        }
        row.setOnClickListener { onClick() }
        row.layoutParams = LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT)
        return row
    }

    /** Gives [row] a button at its end. */
    fun action(row: View, icon: Int, label: String, onClick: () -> Unit) {
        row.findViewById<ImageButton>(R.id.action).apply {
            setImageResource(icon)
            contentDescription = label
            tooltipText = label
            visibility = View.VISIBLE
            setOnClickListener { onClick() }
        }
    }

    /** Asks for a line of text in a Material dialog. */
    fun ask(
        c: Context,
        title: String,
        hint: String,
        ok: String,
        initial: String = "",
        number: Boolean = false,
        password: Boolean = false,
        message: String? = null,
        onCancel: (() -> Unit)? = null,
        onOk: (String) -> Unit,
    ) {
        val field = TextInputEditText(c).apply {
            setText(initial)
            setSelectAllOnFocus(true)
            inputType = when {
                number -> InputType.TYPE_CLASS_NUMBER
                password -> InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
                else -> InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
            }
            isSingleLine = true
        }
        val layout = TextInputLayout(c, null, com.google.android.material.R.attr.textInputOutlinedStyle).apply {
            this.hint = hint
            if (password) endIconMode = TextInputLayout.END_ICON_PASSWORD_TOGGLE
            addView(field)
        }
        val box = FrameLayout(c).apply {
            setPadding(c.dp(24f), c.dp(8f), c.dp(24f), 0)
            addView(layout)
        }
        val builder = MaterialAlertDialogBuilder(c).setTitle(title).setView(box)
            .setPositiveButton(ok) { _, _ -> onOk(field.text.toString()) }
            .setNegativeButton("Cancel") { _, _ -> onCancel?.invoke() }
            .setOnCancelListener { onCancel?.invoke() }
        if (message != null) builder.setMessage(message)
        val dialog = builder.create()
        field.setOnEditorActionListener { _, _, _ ->
            dialog.dismiss()
            onOk(field.text.toString())
            true
        }
        dialog.window?.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_VISIBLE)
        dialog.show()
        field.requestFocus()
    }
}
