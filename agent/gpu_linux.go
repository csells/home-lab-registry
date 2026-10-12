package main

import (
	"context"
	"encoding/binary"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"unsafe"

	"golang.org/x/sys/unix"
)

type gpuEngine struct {
	fd           int
	value, clock uint64
	primed       bool
}

func (e *gpuEngine) read() (float64, bool) {
	b := make([]byte, 24)
	n, err := unix.Read(e.fd, b)
	if err != nil || n != 24 {
		return 0, false
	}
	value := binary.NativeEndian.Uint64(b[:8])
	clock := binary.NativeEndian.Uint64(b[16:24]) // time running accounts for multiplexing
	p, ok := counterPercent(e.value, e.clock, value, clock)
	was := e.primed
	e.value = value
	e.clock = clock
	e.primed = true
	return p, ok && was
}
func readText(p string) string { b, _ := os.ReadFile(p); return strings.TrimSpace(string(b)) }

var cardName = regexp.MustCompile(`^card[0-9]+$`)

func newGPUReader() (func(context.Context) GPU, func()) {
	var paths []string
	var engines []*gpuEngine
	var problems []string
	cards, _ := filepath.Glob("/sys/class/drm/card*")
	physical := 0
	intel := false
	for _, card := range cards {
		if !cardName.MatchString(filepath.Base(card)) {
			continue
		}
		driver, _ := filepath.EvalSymlinks(card + "/device/driver")
		driver = filepath.Base(driver)
		if driver == "bochs-drm" || driver == "qxl" || driver == "simple-framebuffer" || driver == "simpledrm" {
			continue
		}
		physical++
		if driver == "amdgpu" {
			paths = append(paths, card+"/device/gpu_busy_percent")
		} else if driver == "i915" {
			intel = true
		} else {
			problems = append(problems, "GPU driver has no supported utilization counter: "+driver)
		}
	}
	if intel {
		root := "/sys/bus/event_source/devices/i915"
		typ, err := strconv.ParseUint(readText(root+"/type"), 10, 32)
		cpu := 0
		if mask := readText(root + "/cpumask"); mask != "" {
			cpu, _ = strconv.Atoi(strings.FieldsFunc(mask, func(r rune) bool { return r == ',' || r == '-' })[0])
		}
		events, _ := filepath.Glob(root + "/events/*-busy")
		if err != nil || len(events) == 0 {
			problems = append(problems, "Intel GPU PMU unavailable")
		}
		for _, path := range events {
			if err != nil {
				break
			}
			config, e := strconv.ParseUint(strings.TrimPrefix(readText(path), "config="), 0, 64)
			if e != nil || readText(path+".unit") != "ns" {
				problems = append(problems, "Intel GPU counter format unsupported")
				continue
			}
			attr := unix.PerfEventAttr{Type: uint32(typ), Size: uint32(unsafe.Sizeof(unix.PerfEventAttr{})), Config: config, Read_format: unix.PERF_FORMAT_TOTAL_TIME_ENABLED | unix.PERF_FORMAT_TOTAL_TIME_RUNNING}
			fd, e := unix.PerfEventOpen(&attr, -1, cpu, -1, unix.PERF_FLAG_FD_CLOEXEC)
			if e != nil {
				problems = append(problems, "Intel GPU counters require permitted perf access")
				continue
			}
			engine := &gpuEngine{fd: fd}
			engine.read()
			engines = append(engines, engine)
		}
	}
	// A PCI display controller without a usable driver is unknown, not absent.
	if physical == 0 {
		pci, _ := filepath.Glob("/sys/bus/pci/devices/*/class")
		for _, p := range pci {
			if strings.HasPrefix(readText(p), "0x03") {
				driver, _ := filepath.EvalSymlinks(filepath.Dir(p) + "/driver")
				driver = filepath.Base(driver)
				if driver == "bochs-drm" || driver == "qxl" {
					continue
				}
				physical++
				problems = append(problems, "Display hardware detected without supported GPU counters")
			}
		}
	}
	close := func() {
		for _, e := range engines {
			unix.Close(e.fd)
		}
	}
	return func(context.Context) GPU {
		if physical == 0 {
			return GPU{Status: "not_applicable", Note: "No GPU accelerator exposed by this OS"}
		}
		if len(problems) > 0 {
			return GPU{Status: "unavailable", Note: problems[0]}
		}
		max := -1.0
		for _, p := range paths {
			raw, e := os.ReadFile(p)
			if e != nil {
				return GPU{Status: "unavailable", Note: "AMD GPU counter unreadable"}
			}
			v, e := strconv.ParseFloat(strings.TrimSpace(string(raw)), 64)
			if e != nil || !validGPUPercent(v) {
				return GPU{Status: "unavailable", Note: "Invalid AMD GPU counter"}
			}
			if v > max {
				max = v
			}
		}
		for _, e := range engines {
			v, ok := e.read()
			if !ok {
				return GPU{Status: "unavailable", Note: "Intel GPU counter warming up or reset"}
			}
			if v > max {
				max = v
			}
		}
		if max < 0 {
			return GPU{Status: "unavailable", Note: "GPU utilization unavailable"}
		}
		return gpuValue(max, fmt.Sprintf("Busiest GPU engine/device (%d measured)", len(paths)+len(engines)))
	}, close
}
