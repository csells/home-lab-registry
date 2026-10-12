package main

import (
	"math"
	"regexp"
	"strconv"
)

// Percent is never synthesized for missing hardware or unavailable counters.
// For multiple engines/devices it is the busiest measured engine/device.
type GPU struct {
	Status  string   `json:"status"`
	Percent *float64 `json:"percent"`
	Note    string   `json:"note,omitempty"`
}

func validGPUPercent(v float64) bool {
	return !math.IsNaN(v) && !math.IsInf(v, 0) && v >= 0 && v <= 100
}
func gpuValue(v float64, note string) GPU {
	if !validGPUPercent(v) {
		return GPU{Status: "unavailable", Note: "GPU counter outside expected range"}
	}
	return GPU{Status: "available", Percent: &v, Note: note}
}
func counterPercent(old, oldTime, value, clock uint64) (float64, bool) {
	if clock <= oldTime || value < old {
		return 0, false
	}
	p := 100 * float64(value-old) / float64(clock-oldTime)
	return p, validGPUPercent(p)
}

var appleUtil = regexp.MustCompile(`"Device Utilization %"\s*=\s*([0-9.]+)`)

func parseAppleGPU(raw []byte) GPU {
	if len(raw) == 0 {
		return GPU{Status: "not_applicable", Note: "No GPU accelerator exposed by this OS"}
	}
	matches := appleUtil.FindAllSubmatch(raw, -1)
	max := -1.0
	for _, m := range matches {
		v, e := strconv.ParseFloat(string(m[1]), 64)
		if e != nil || !validGPUPercent(v) {
			return GPU{Status: "unavailable", Note: "Invalid Apple GPU counter"}
		}
		if v > max {
			max = v
		}
	}
	if max < 0 {
		return GPU{Status: "unavailable", Note: "GPU detected; utilization counter unavailable"}
	}
	return gpuValue(max, "Apple device utilization; busiest GPU")
}
