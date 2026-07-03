package org.srx.manager

import android.app.Application
import android.content.Context
import android.content.pm.ApplicationInfo
import android.os.Build
import org.lsposed.hiddenapibypass.HiddenApiBypass

private const val SharedPrefsName = "srx_manager"
private const val PredictiveBackPrefsKey = "predictive_back"

class SrxManagerApp : Application() {
    companion object {
        fun setEnableOnBackInvokedCallback(appInfo: ApplicationInfo, enable: Boolean) {
            runCatching {
                val method = ApplicationInfo::class.java.getDeclaredMethod(
                    "setEnableOnBackInvokedCallback",
                    Boolean::class.javaPrimitiveType,
                )
                method.isAccessible = true
                method.invoke(appInfo, enable)
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            val enabled = getSharedPreferences(SharedPrefsName, Context.MODE_PRIVATE)
                .getBoolean(PredictiveBackPrefsKey, false)
            syncPredictiveBackEnabled(applicationInfo, enabled)
        }
    }
}

fun syncPredictiveBackEnabled(appInfo: ApplicationInfo, enabled: Boolean) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) return
    HiddenApiBypass.addHiddenApiExemptions("Landroid/content/pm/ApplicationInfo;->setEnableOnBackInvokedCallback")
    SrxManagerApp.setEnableOnBackInvokedCallback(appInfo, enabled)
}
