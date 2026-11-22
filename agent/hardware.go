package main

import (
	"runtime"
	"sync"
	"time"

	"github.com/shirou/gopsutil/v4/cpu"
	"github.com/shirou/gopsutil/v4/mem"
)

type hardwareReading struct {
	OS           string  `json:"os"`
	Architecture string  `json:"architecture"`
	LogicalCPUs  int     `json:"logical_cpus"`
	MemoryBytes  uint64  `json:"memory_bytes"`
	Checked      float64 `json:"checked"`
}

// Reuse measurements across guest events and subscribers. A failed read keeps
// the last successful timestamp so consumers can distinguish stale evidence.
func hardwareCollector() func() *hardwareReading {
	var mu sync.Mutex
	var last *hardwareReading
	var next time.Time
	return func() *hardwareReading {
		mu.Lock()
		defer mu.Unlock()
		now := time.Now()
		if now.Before(next) {
			return last
		}
		next = now.Add(10 * time.Minute)
		count, cpuErr := cpu.Counts(true)
		memory, memErr := mem.VirtualMemory()
		if cpuErr != nil || memErr != nil || count < 1 || memory.Total == 0 {
			next = now.Add(time.Minute)
			return last
		}
		last = &hardwareReading{
			OS: runtime.GOOS, Architecture: runtime.GOARCH, LogicalCPUs: count,
			MemoryBytes: memory.Total, Checked: timestamp(),
		}
		return last
	}
}
