package main

import "testing"

func TestNVIDIAReadings(t *testing.T) {
	for _, tc := range []struct {
		name   string
		values []uint32
		code   int32
		status string
		want   float64
	}{
		{"idle", []uint32{0}, 0, "available", 0},
		{"busiest", []uint32{12, 93}, 0, "available", 93},
		{"lost", []uint32{0}, 15, "unavailable", 0},
		{"invalid", []uint32{101}, 0, "unavailable", 0},
		{"none", nil, 0, "unavailable", 0},
	} {
		t.Run(tc.name, func(t *testing.T) {
			api := nvmlAPI{count: func(p *uint32) int32 { *p = uint32(len(tc.values)); return 0 }, handle: func(i uint32, p *uintptr) int32 { *p = uintptr(i); return 0 }, util: func(h uintptr, u *nvmlUtilization) int32 { u.GPU = tc.values[h]; return tc.code }}
			g := api.read()
			if g.Status != tc.status {
				t.Fatalf("%+v", g)
			}
			if tc.status == "available" && (g.Percent == nil || *g.Percent != tc.want) {
				t.Fatalf("%+v", g)
			}
			if tc.status != "available" && g.Percent != nil {
				t.Fatal("failure became a numeric reading")
			}
		})
	}
}
func TestNVIDIAEnumerationErrors(t *testing.T) {
	a := nvmlAPI{count: func(*uint32) int32 { return 9 }}
	if a.read().Status != "unavailable" {
		t.Fatal("count error hidden")
	}
	a = nvmlAPI{count: func(p *uint32) int32 { *p = 1; return 0 }, handle: func(uint32, *uintptr) int32 { return 15 }}
	if a.read().Status != "unavailable" {
		t.Fatal("handle error hidden")
	}
}
