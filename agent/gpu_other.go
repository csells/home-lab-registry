//go:build !linux && !darwin

package main

import "context"

func newGPUReader() (func(context.Context) GPU, func()) {
	return func(context.Context) GPU {
		return GPU{Status: "unavailable", Note: "GPU monitoring not implemented for this OS"}
	}, func() {}
}
