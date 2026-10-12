package main

import (
	"math"
	"testing"
)

func TestGPUPercentRejectsBadReadings(t *testing.T) {
	for _, v := range []float64{-1, 101, math.NaN(), math.Inf(1)} {
		if validGPUPercent(v) {
			t.Fatalf("accepted %v", v)
		}
	}
	if !validGPUPercent(0) || !validGPUPercent(100) {
		t.Fatal("boundary values rejected")
	}
}
func TestAppleGPUCapturedStatistics(t *testing.T) {
	raw := []byte(`+-o AGXAcceleratorG16G <class AGXAcceleratorG16G>
 "PerformanceStatistics" = {"Tiler Utilization %"=11,"Renderer Utilization %"=10,"Device Utilization %"=11}
 "model" = "Apple M4"`)
	g := parseAppleGPU(raw)
	if g.Status != "available" || g.Percent == nil || *g.Percent != 11 {
		t.Fatalf("%+v", g)
	}
	if parseAppleGPU([]byte(`+-o AGXAccelerator <class AGX>`)).Status != "unavailable" {
		t.Fatal("missing counter called no GPU")
	}
	if parseAppleGPU([]byte("")).Status != "not_applicable" {
		t.Fatal("no accelerator not recognized")
	}
	if parseAppleGPU([]byte(`"Device Utilization %"=101`)).Status != "unavailable" {
		t.Fatal("bad counter accepted")
	}
}
func TestGPUCounterIntervals(t *testing.T) {
	p, ok := counterPercent(100, 1000, 300, 2000)
	if !ok || p != 20 {
		t.Fatalf("%v %v", p, ok)
	}
	for _, v := range [][4]uint64{{100, 1000, 90, 2000}, {100, 1000, 200, 1000}, {0, 0, 2000, 1000}} {
		if _, ok := counterPercent(v[0], v[1], v[2], v[3]); ok {
			t.Fatal("reset/stalled/impossible counter accepted")
		}
	}
}
