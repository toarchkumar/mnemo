import Foundation
import UIKit

/// The device and build fields every Benchmark JSON records (design note Q7).
enum DeviceInfo {
    /// `iPhone16,1` and so on (`hw.machine`); `arm64` or `x86_64` on the
    /// simulator, with the simulated model in `simulator_model_identifier`.
    static var modelIdentifier: String {
        var info = utsname()
        uname(&info)
        return withUnsafeBytes(of: &info.machine) { raw in
            String(decoding: raw.prefix(while: { $0 != 0 }), as: UTF8.self)
        }
    }

    static func sysctlString(_ name: String) -> String? {
        var size = 0
        guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 0 else { return nil }
        var bytes = [CChar](repeating: 0, count: size)
        guard sysctlbyname(name, &bytes, &size, nil, 0) == 0 else { return nil }
        return String(cString: bytes)
    }

    /// The SoC for the model identifiers this table knows; `nil` otherwise
    /// (the identifier names the device, so it can be looked up later).
    static func soc(for identifier: String) -> String? {
        let table: [(String, String)] = [
            ("iPhone13,", "A14 Bionic"),
            ("iPhone14,2", "A15 Bionic"), ("iPhone14,3", "A15 Bionic"), ("iPhone14,4", "A15 Bionic"),
            ("iPhone14,5", "A15 Bionic"), ("iPhone14,6", "A15 Bionic"), ("iPhone14,7", "A15 Bionic"),
            ("iPhone14,8", "A15 Bionic"),
            ("iPhone15,2", "A16 Bionic"), ("iPhone15,3", "A16 Bionic"), ("iPhone15,4", "A16 Bionic"),
            ("iPhone15,5", "A16 Bionic"),
            ("iPhone16,1", "A17 Pro"), ("iPhone16,2", "A17 Pro"),
            ("iPhone17,1", "A18 Pro"), ("iPhone17,2", "A18 Pro"),
            ("iPhone17,3", "A18"), ("iPhone17,4", "A18"), ("iPhone17,5", "A18"),
        ]
        return table.first { identifier == $0.0 || ($0.0.hasSuffix(",") && identifier.hasPrefix($0.0)) }?.1
    }

    /// The process's memory footprint in KB (`task_vm_info.phys_footprint`,
    /// the number iOS's memory limit is enforced on).
    static func footprintKB() -> UInt64 {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        return kr == KERN_SUCCESS ? info.phys_footprint / 1024 : 0
    }

    /// Build fields written by the app target's build script
    /// (`BuildInfo.json` in the bundle).
    static var build: [String: Any] {
        var fields: [String: Any] = [:]
        if let url = Bundle.main.url(forResource: "BuildInfo", withExtension: "json"),
           let data = try? Data(contentsOf: url),
           let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            fields = json
        }
        #if DEBUG
        fields["release_build"] = false
        #else
        fields["release_build"] = true
        #endif
        return fields
    }

    /// Everything Q7 asks for, read now.
    @MainActor static func snapshot() -> [String: Any] {
        let device = UIDevice.current
        device.isBatteryMonitoringEnabled = true
        let identifier = modelIdentifier
        let env = ProcessInfo.processInfo.environment
        let simulated = env["SIMULATOR_MODEL_IDENTIFIER"]
        let home = URL(fileURLWithPath: NSHomeDirectory())
        let free = (try? home.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]))?
            .volumeAvailableCapacityForImportantUsage
        let battery: String
        switch device.batteryState {
        case .charging: battery = "charging"
        case .full: battery = "full"
        case .unplugged: battery = "unplugged"
        default: battery = "unknown"
        }
        let thermal: String
        switch ProcessInfo.processInfo.thermalState {
        case .nominal: thermal = "nominal"
        case .fair: thermal = "fair"
        case .serious: thermal = "serious"
        case .critical: thermal = "critical"
        @unknown default: thermal = "unknown"
        }
        var fields: [String: Any] = [
            "platform": "ios",
            "model_identifier": identifier,
            "model_name": device.model,
            "ram_bytes": ProcessInfo.processInfo.physicalMemory,
            "cpu_count": ProcessInfo.processInfo.activeProcessorCount,
            "os": device.systemName,
            "os_version": device.systemVersion,
            "os_build": sysctlString("kern.osversion") ?? NSNull(),
            "page_size": Int(getpagesize()),
            "free_storage_bytes": free.map { NSNumber(value: $0) } ?? NSNull(),
            "battery_level": device.batteryLevel < 0 ? NSNull() : NSNumber(value: device.batteryLevel),
            "battery_state": battery,
            "thermal_state": thermal,
            "low_power_mode": ProcessInfo.processInfo.isLowPowerModeEnabled,
            "simulator": simulated != nil,
        ]
        fields["soc"] = soc(for: simulated ?? identifier) ?? NSNull()
        if let simulated { fields["simulator_model_identifier"] = simulated }
        return fields
    }
}
