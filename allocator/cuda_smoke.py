"""Check that a CUDA guest can allocate, write and read real device memory."""
import ctypes
import json


def check(result, operation):
    if result != 0:
        raise RuntimeError(f'{operation} returned CUDA error {result}')


def main():
    cuda = ctypes.CDLL('libcuda.so.1')
    methods = {
        'cuInit': [ctypes.c_uint],
        'cuDeviceGet': [ctypes.POINTER(ctypes.c_int), ctypes.c_int],
        'cuDevicePrimaryCtxRetain': [ctypes.POINTER(ctypes.c_void_p), ctypes.c_int],
        'cuDevicePrimaryCtxRelease_v2': [ctypes.c_int],
        'cuCtxSetCurrent': [ctypes.c_void_p],
        'cuMemAlloc_v2': [ctypes.POINTER(ctypes.c_uint64), ctypes.c_size_t],
        'cuMemFree_v2': [ctypes.c_uint64],
        'cuMemsetD32_v2': [ctypes.c_uint64, ctypes.c_uint, ctypes.c_size_t],
        'cuMemcpyDtoH_v2': [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_size_t],
    }

    def invoke(name, *args):
        method = getattr(cuda, name)
        method.argtypes, method.restype = methods[name], ctypes.c_int
        check(method(*args), name)

    device, context = ctypes.c_int(), ctypes.c_void_p()
    invoke('cuInit', 0)
    invoke('cuDeviceGet', ctypes.byref(device), 0)
    invoke('cuDevicePrimaryCtxRetain', ctypes.byref(context), device)
    try:
        invoke('cuCtxSetCurrent', context)
        expected = 0x8C37AA21
        destination = (ctypes.c_uint32 * 256)()
        address = ctypes.c_uint64()
        size = ctypes.sizeof(destination)
        invoke('cuMemAlloc_v2', ctypes.byref(address), size)
        try:
            invoke('cuMemsetD32_v2', address, expected, len(destination))
            invoke('cuMemcpyDtoH_v2', destination, address, size)
            if any(word != expected for word in destination):
                raise RuntimeError('GPU readback differs from the value written')
        finally:
            invoke('cuMemFree_v2', address)
    finally:
        invoke('cuCtxSetCurrent', None)
        invoke('cuDevicePrimaryCtxRelease_v2', device)
    print(json.dumps({'device': device.value, 'bytes_verified': size, 'cuda': 'verified'}))


if __name__ == '__main__':
    main()
