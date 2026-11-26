// AirPods head tracker → OpenTrack UDP on 127.0.0.1.
//
// Reads Apple's headphone motion (CoreMotion `CMHeadphoneMotionManager`,
// macOS 14+) and sends every sample as one OpenTrack packet: six
// little-endian doubles, x y z (cm, always 0) then yaw pitch roll (degrees).
// Built and wrapped as an app by scripts/airpods-head-tracker.sh; macOS only
// grants motion access to an app bundle with NSMotionUsageDescription.
//
// Usage: AirPodsBridge bridge [--port 4242]   stream until terminated
//        AirPodsBridge probe                  report whether motion arrives

import CoreMotion
import Foundation

/// Sign that turns Apple's yaw into the convention the engine expects from
/// every tracker: a LEFT turn is NEGATIVE degrees (`TRACKER_YAW_SIGN` in
/// src/head_tracking/opentrack.rs). CoreMotion's attitude grows
/// counter-clockwise seen from above, so a left turn is positive and is
/// flipped here.
let yawSign = -1.0

func say(_ line: String) {
    FileHandle.standardOutput.write((line + "\n").data(using: .utf8)!)
}

func degrees(_ radians: Double) -> Double { radians * 180.0 / Double.pi }

/// A UDP socket aimed at the engine's OpenTrack listener on loopback.
final class OpenTrackSender {
    private let socketDescriptor: Int32
    private var address = sockaddr_in()

    init(port: UInt16) {
        socketDescriptor = socket(AF_INET, SOCK_DGRAM, 0)
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = port.bigEndian
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
    }

    func send(yawDegrees: Double, pitchDegrees: Double, rollDegrees: Double) {
        var packet = Data(capacity: 48)
        for value in [0.0, 0.0, 0.0, yawDegrees, pitchDegrees, rollDegrees] {
            withUnsafeBytes(of: value.bitPattern.littleEndian) { packet.append(contentsOf: $0) }
        }
        packet.withUnsafeBytes { bytes in
            withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { socketAddress in
                    _ = sendto(
                        socketDescriptor, bytes.baseAddress, bytes.count, 0,
                        socketAddress, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
        }
    }
}

final class Connection: NSObject, CMHeadphoneMotionManagerDelegate {
    func headphoneMotionManagerDidConnect(_ manager: CMHeadphoneMotionManager) {
        say("AirPods connected")
    }

    func headphoneMotionManagerDidDisconnect(_ manager: CMHeadphoneMotionManager) {
        say("AirPods disconnected: waiting for them to come back")
    }
}

func authorizationName(_ status: CMAuthorizationStatus) -> String {
    switch status {
    case .notDetermined: return "not asked yet"
    case .restricted: return "restricted"
    case .denied: return "denied"
    case .authorized: return "allowed"
    @unknown default: return "unknown"
    }
}

let arguments = CommandLine.arguments.dropFirst()
let command = arguments.first ?? "bridge"
var port: UInt16 = 4242
if let index = arguments.firstIndex(of: "--port"), arguments.index(after: index) < arguments.endIndex,
    let value = UInt16(arguments[arguments.index(after: index)])
{
    port = value
}

let manager = CMHeadphoneMotionManager()
let connection = Connection()
manager.delegate = connection

say("Motion access: \(authorizationName(CMHeadphoneMotionManager.authorizationStatus()))")
guard manager.isDeviceMotionAvailable else {
    say("Headphone motion is not available on this Mac (needs macOS 14 or later).")
    exit(1)
}

let sender = OpenTrackSender(port: port)
let queue = OperationQueue()
queue.maxConcurrentOperationCount = 1
var samples = 0
var firstSample: Date?
var lastReport = Date()
let started = Date()

manager.startDeviceMotionUpdates(to: queue) { motion, error in
    if let error = error {
        say("Motion error: \(error.localizedDescription)")
        return
    }
    guard let attitude = motion?.attitude else { return }
    let yaw = yawSign * degrees(attitude.yaw)
    let pitch = degrees(attitude.pitch)
    let roll = degrees(attitude.roll)
    samples += 1
    if command == "probe" {
        if samples == 1 {
            firstSample = Date()
            say("First sample after \(String(format: "%.1f", Date().timeIntervalSince(started))) s")
        }
        return
    }
    sender.send(yawDegrees: yaw, pitchDegrees: pitch, rollDegrees: roll)
    if samples == 1 {
        say("Streaming head tracking to 127.0.0.1:\(port)")
    }
    if Date().timeIntervalSince(lastReport) >= 10 {
        say(String(format: "%d samples, yaw %.0f°", samples, yaw))
        lastReport = Date()
    }
}

for signalNumber in [SIGTERM, SIGINT] {
    signal(signalNumber, SIG_IGN)
    let source = DispatchSource.makeSignalSource(signal: signalNumber, queue: .main)
    source.setEventHandler {
        manager.stopDeviceMotionUpdates()
        say("Stopped after \(samples) samples")
        exit(0)
    }
    source.resume()
    // Keep the source alive for the life of the process.
    _ = Unmanaged.passRetained(source)
}

if command == "probe" {
    DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
        manager.stopDeviceMotionUpdates()
        let streaming = firstSample.map { Date().timeIntervalSince($0) } ?? 0
        let rate = streaming > 0 ? Double(samples) / streaming : 0
        say(String(format: "%d samples in 5 s (%.0f per second once streaming)", samples, rate))
        say("Motion access: \(authorizationName(CMHeadphoneMotionManager.authorizationStatus()))")
        if samples == 0 {
            say("No motion arrived. Are the AirPods in your ears and connected to this Mac?")
        }
        exit(samples > 0 ? 0 : 1)
    }
}

dispatchMain()
