import Metal

guard let device = MTLCreateSystemDefaultDevice() else {
    fatalError("No Metal device in this guest.")
}
let source = """
#include <metal_stdlib>
using namespace metal;
kernel void verify(device uint *values [[buffer(0)]],
                   uint position [[thread_position_in_grid]]) {
    values[position] = position * 3 + 11;
}
"""
let library = try device.makeLibrary(source: source, options: nil)
guard let function = library.makeFunction(name: "verify"),
      let queue = device.makeCommandQueue(),
      let buffer = device.makeBuffer(length: 4 * MemoryLayout<UInt32>.stride, options: .storageModeShared),
      let command = queue.makeCommandBuffer(),
      let encoder = command.makeComputeCommandEncoder() else {
    fatalError("Could not create a Metal compute command.")
}
let pipeline = try device.makeComputePipelineState(function: function)
encoder.setComputePipelineState(pipeline)
encoder.setBuffer(buffer, offset: 0, index: 0)
encoder.dispatchThreads(MTLSize(width: 4, height: 1, depth: 1),
                        threadsPerThreadgroup: MTLSize(width: 4, height: 1, depth: 1))
encoder.endEncoding()
command.commit()
command.waitUntilCompleted()
guard command.status == .completed else {
    fatalError("Metal command failed: \(String(describing: command.error))")
}
let values = buffer.contents().assumingMemoryBound(to: UInt32.self)
guard Array(UnsafeBufferPointer(start: values, count: 4)) == [11, 14, 17, 20] else {
    fatalError("Unexpected Metal result.")
}
print("metal-compute-verified: \(device.name)")
