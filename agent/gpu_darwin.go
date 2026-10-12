package main

import (
	"bytes"
	"context"
	"os/exec"
	"time"
)

func newGPUReader() (func(context.Context) GPU, func()) {
	return func(parent context.Context) GPU {
		ctx, cancel := context.WithTimeout(parent, 2*time.Second)
		defer cancel()
		raw, err := exec.CommandContext(ctx, "/usr/sbin/ioreg", "-r", "-d", "1", "-c", "IOAccelerator").Output()
		if err != nil {
			return GPU{Status: "unavailable", Note: "Cannot read Apple GPU statistics"}
		}
		return parseAppleGPU(bytes.TrimSpace(raw))
	}, func() {}
}
