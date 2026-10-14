package main

import (
	"fmt"
	"log"
	"sync"
	"time"

	"github.com/ebitengine/purego"
)

// NVML's stable C ABI uses unsigned int counters and opaque device pointers.
// Open it once; sampling does not fork nvidia-smi or require the CUDA toolkit.
type nvmlUtilization struct{ GPU, Memory uint32 }
type nvmlAPI struct {
	init     func() int32
	shutdown func() int32
	count    func(*uint32) int32
	handle   func(uint32, *uintptr) int32
	util     func(uintptr, *nvmlUtilization) int32
}

func (a nvmlAPI) read() GPU {
	fail := func(op string, code int32) GPU {
		return GPU{Status: "unavailable", Note: fmt.Sprintf("NVIDIA NVML %s failed (code %d)", op, code)}
	}
	var count uint32
	if code := a.count(&count); code != 0 {
		return fail("enumeration", code)
	}
	if count == 0 {
		return GPU{Status: "unavailable", Note: "NVIDIA NVML exposes no accessible devices"}
	}
	max := 0.0
	for i := uint32(0); i < count; i++ {
		var device uintptr
		if code := a.handle(i, &device); code != 0 {
			return fail("device access", code)
		}
		var u nvmlUtilization
		if code := a.util(device, &u); code != 0 {
			return fail("utilization", code)
		}
		v := float64(u.GPU)
		if !validGPUPercent(v) {
			return GPU{Status: "unavailable", Note: "Invalid NVIDIA utilization counter"}
		}
		if v > max {
			max = v
		}
	}
	return gpuValue(max, fmt.Sprintf("NVIDIA NVML; busiest of %d GPU devices", count))
}
func openNVML() (func() GPU, func(), error) {
	lib, err := purego.Dlopen("libnvidia-ml.so.1", purego.RTLD_NOW|purego.RTLD_LOCAL)
	if err != nil {
		return nil, nil, fmt.Errorf("NVIDIA NVML library unavailable: %w", err)
	}
	unload := func() {
		if err := purego.Dlclose(lib); err != nil {
			log.Printf("NVML unload: %v", err)
		}
	}
	a := nvmlAPI{}
	for _, s := range []struct {
		name string
		fn   any
	}{
		{"nvmlInit_v2", &a.init}, {"nvmlShutdown", &a.shutdown},
		{"nvmlDeviceGetCount_v2", &a.count}, {"nvmlDeviceGetHandleByIndex_v2", &a.handle},
		{"nvmlDeviceGetUtilizationRates", &a.util},
	} {
		symbol, e := purego.Dlsym(lib, s.name)
		if e != nil {
			unload()
			return nil, nil, fmt.Errorf("NVIDIA NVML missing %s: %w", s.name, e)
		}
		purego.RegisterFunc(s.fn, symbol)
	}
	if code := a.init(); code != 0 {
		unload()
		return nil, nil, fmt.Errorf("NVIDIA NVML initialization failed (code %d)", code)
	}
	var once sync.Once
	close := func() {
		once.Do(func() {
			if code := a.shutdown(); code != 0 {
				log.Printf("NVML shutdown failed (code %d)", code)
			}
			unload()
		})
	}
	return a.read, close, nil
}

// Drivers can become ready after the daemon starts. Retry initialization without
// changing unavailable readings into synthetic zeroes or requiring a restart.
func newNVMLReader() (func() GPU, func()) {
	var read func() GPU
	var release func()
	var retry time.Time
	var initErr error
	return func() GPU {
		if read == nil && !time.Now().Before(retry) {
			read, release, initErr = openNVML()
			retry = time.Now().Add(30 * time.Second)
		}
		if read == nil {
			return GPU{Status: "unavailable", Note: initErr.Error()}
		}
		return read()
	}, func() {
		if release != nil {
			release()
		}
	}
}
