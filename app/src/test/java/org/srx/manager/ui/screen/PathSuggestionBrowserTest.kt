package org.srx.manager.ui.screen

import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test

class PathSuggestionBrowserTest {
    @Test
    fun androidInputExpandsAndroidChildren() {
        val parsed = splitPathBrowserInput("Android", "0")
        val suggestions = pathBrowserSuggestions(
            parsed,
            listOf("data/", "media/", "obb/"),
        )

        assertEquals("Android", parsed.dirRel)
        assertEquals("Android/", parsed.prefix)
        assertEquals(
            listOf("..", "data", "media", "obb"),
            suggestions.map { it.displayPath },
        )
        assertEquals(
            listOf("", "Android/data/", "Android/media/", "Android/obb/"),
            suggestions.map { it.relativePath },
        )
    }

    @Test
    fun androidInputNormalizesCase() {
        val parsed = splitPathBrowserInput("android", "0")

        assertEquals("Android", parsed.dirRel)
        assertEquals("Android/", parsed.prefix)
    }

    @Test
    fun normalizedTextFieldValuePreservesSelectionChanges() {
        val selected = TextFieldValue(
            text = "Pictures/Screenshots",
            selection = TextRange(0, "Pictures/Screenshots".length),
        )

        val normalized = normalizeEditablePathTextFieldValue(selected, "0")

        assertSame(selected, normalized)
        assertEquals(TextRange(0, "Pictures/Screenshots".length), normalized.selection)
    }

    @Test
    fun normalizedTextFieldValueClampsSelectionAfterTextCleanup() {
        val absolute = TextFieldValue(
            text = "/storage/emulated/0/Pictures",
            selection = TextRange(0, "/storage/emulated/0/Pictures".length),
        )

        val normalized = normalizeEditablePathTextFieldValue(absolute, "0")

        assertEquals("Pictures", normalized.text)
        assertEquals(TextRange(0, "Pictures".length), normalized.selection)
    }
}
