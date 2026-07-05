package me.fakerqu.media_store_api

import java.io.File
import java.io.InputStream
import java.io.OutputStream

data class FileStatInfo(
    val size: Long,
    val mode: Int,
    val uid: Int,
    val gid: Int,
    val modifiedSeconds: Long,
)

interface IFileApi {
    fun getDirFilesRecursive(dir: String): List<File>

    fun readFile(file: String): InputStream

    fun writeFile(file: String): OutputStream

    fun createFile(path: String): File

    fun deleteFile(path: String): Boolean

    fun mkdir(path: String): Boolean

    fun renameFile(fromPath: String, toPath: String): Boolean

    fun statFile(path: String): FileStatInfo

    fun accessFile(path: String, mode: Int): Boolean

    fun readLink(path: String): String

    fun truncateFile(path: String, length: Long)

    fun ftruncateFile(path: String, length: Long)

    fun chmodFile(path: String, mode: Int)

    fun fchmodFile(path: String, mode: Int)

    fun linkFile(fromPath: String, toPath: String)

    fun symlinkFile(targetPath: String, linkPath: String)
}
