package io.github.toarchkumar.mnemo.sample

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.os.Build
import android.os.Debug
import android.os.PowerManager
import android.os.StatFs
import android.system.Os
import android.system.OsConstants
import java.io.File
import org.json.JSONObject

/** The device and build fields every Benchmark JSON records (design note Q7). */
object DeviceInfo {
    /** Total PSS of this process in KB (`Debug.MemoryInfo`). */
    fun pssKb(): Long {
        val info = Debug.MemoryInfo()
        Debug.getMemoryInfo(info)
        return info.totalPss.toLong()
    }

    /** `VmRSS` from `/proc/self/status`, in KB. */
    fun rssKb(): Long = try {
        File("/proc/self/status").readLines()
            .firstOrNull { it.startsWith("VmRSS:") }
            ?.split(Regex("\\s+"))?.getOrNull(1)?.toLong() ?: 0L
    } catch (_: Exception) {
        0L
    }

    fun pageSize(): Long = Os.sysconf(OsConstants._SC_PAGESIZE)

    fun build(): JSONObject = JSONObject()
        .put("mnemo_version", BuildConfig.MNEMO_VERSION)
        .put("commit", BuildConfig.MNEMO_COMMIT)
        .put("configuration", if (BuildConfig.DEBUG) "debug" else "release")
        .put("release_build", !BuildConfig.DEBUG)

    fun snapshot(context: Context): JSONObject {
        val memory = ActivityManager.MemoryInfo()
        (context.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager).getMemoryInfo(memory)
        val power = context.getSystemService(Context.POWER_SERVICE) as PowerManager
        val battery = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val level = battery?.let {
            val l = it.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
            val scale = it.getIntExtra(BatteryManager.EXTRA_SCALE, -1)
            if (l >= 0 && scale > 0) l.toDouble() / scale else null
        }
        val status = battery?.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
        val batteryState = when (status) {
            BatteryManager.BATTERY_STATUS_CHARGING -> "charging"
            BatteryManager.BATTERY_STATUS_FULL -> "full"
            BatteryManager.BATTERY_STATUS_DISCHARGING, BatteryManager.BATTERY_STATUS_NOT_CHARGING -> "unplugged"
            else -> "unknown"
        }
        val thermal = if (Build.VERSION.SDK_INT >= 29) {
            when (power.currentThermalStatus) {
                PowerManager.THERMAL_STATUS_NONE -> "none"
                PowerManager.THERMAL_STATUS_LIGHT -> "light"
                PowerManager.THERMAL_STATUS_MODERATE -> "moderate"
                PowerManager.THERMAL_STATUS_SEVERE -> "severe"
                PowerManager.THERMAL_STATUS_CRITICAL -> "critical"
                PowerManager.THERMAL_STATUS_EMERGENCY -> "emergency"
                PowerManager.THERMAL_STATUS_SHUTDOWN -> "shutdown"
                else -> "unknown"
            }
        } else {
            "unknown (API < 29)"
        }
        val stat = StatFs(context.filesDir.path)
        return JSONObject()
            .put("platform", "android")
            .put("model_identifier", Build.MODEL)
            .put("manufacturer", Build.MANUFACTURER)
            .put("device", Build.DEVICE)
            .put("soc", if (Build.VERSION.SDK_INT >= 31) "${Build.SOC_MANUFACTURER} ${Build.SOC_MODEL}" else Build.HARDWARE)
            .put("abi", Build.SUPPORTED_ABIS.firstOrNull())
            .put("ram_bytes", memory.totalMem)
            .put("cpu_count", Runtime.getRuntime().availableProcessors())
            .put("os", "Android")
            .put("os_version", Build.VERSION.RELEASE)
            .put("sdk_int", Build.VERSION.SDK_INT)
            .put("os_build", Build.DISPLAY)
            .put("os_fingerprint", Build.FINGERPRINT)
            .put("page_size", pageSize())
            .put("free_storage_bytes", stat.availableBytes)
            .put("battery_level", level ?: JSONObject.NULL)
            .put("battery_state", batteryState)
            .put("thermal_state", thermal)
            .put("low_power_mode", power.isPowerSaveMode)
            .put("emulator", Build.FINGERPRINT.contains("generic") || Build.HARDWARE.contains("ranchu"))
    }
}
