package org.srx.manager.root

import java.util.Base64

private val safePackage = Regex("^[A-Za-z0-9_.-]+$")
private val safeUser = Regex("^[0-9]+$")

fun shellQuote(value: String): String = "'" + value.replace("'", "'\\''") + "'"

fun isSafePackageName(value: String): Boolean = safePackage.matches(value)

fun isSafeUserId(value: String): Boolean = safeUser.matches(value)

fun base64Utf8(value: String): String =
    Base64.getEncoder().encodeToString(value.toByteArray(Charsets.UTF_8))
