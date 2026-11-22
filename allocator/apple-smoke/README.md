# Apple test workloads

Use these small fixtures to qualify the toolchain, simulator input and Metal
compute on an approved Apple environment. The fixtures were developed for the
home lab's registered Tart recipe. The same simulator project can run directly
on a Mac with Xcode. The fixtures contain no worker or signing configuration.

Run the UI test on a task-owned simulator by supplying its UDID and a private
output directory. Shut down that simulator after the check.

```sh
xcodebuild -project simulator-smoke/Smoke.xcodeproj -scheme Smoke \
  -destination "platform=iOS Simulator,id=$SIMULATOR_UDID" \
  -parallel-testing-enabled NO -resultBundlePath "$EVIDENCE/Smoke.xcresult" test
```

Verify that XCTest tapped the app's button and checked the changed label. Save
the result bundle before releasing an environment.

For a lease that requires Metal, compile and run the separate compute workload.

```sh
xcrun swiftc MetalSmoke.swift -o "$EVIDENCE/metal-smoke"
"$EVIDENCE/metal-smoke"
```

Require the expected compute result on the actual target. A simulator test does
not establish physical-device, audio, or camera availability.
