package main

import (
	"testing"
	"time"
)

func TestRingKeepsHistoryWithoutSubscribers(t *testing.T) {
	s := newStore(120)
	for i := 0; i < 130; i++ {
		s.add(Sample{Checked: float64(i), Status: "available"})
	}
	history, _, _ := s.snapshot()
	if len(history) != 120 || history[0].Checked != 10 || history[119].Checked != 129 {
		t.Fatalf("bad bounded history: %v", history)
	}
}
func TestSubscribersWakeOnNewSample(t *testing.T) {
	s := newStore(120)
	_, changed, _ := s.snapshot()
	s.add(Sample{Checked: 1})
	select {
	case <-changed:
	case <-time.After(time.Second):
		t.Fatal("subscriber never notified")
	}
}
func TestThinPoolCapacity(t *testing.T) {
	data := []byte(`{"report":[{"lv":[{"lv_name":"data","vg_name":"vg","lv_size":"1000","data_percent":"25.5","metadata_percent":"2","segtype":"thin-pool"},{"lv_name":"guest","vg_name":"vg","lv_size":"2000","data_percent":"90","metadata_percent":"","segtype":"thin"}]}]}`)
	disks, err := parseThinPools(data, 100)
	if err != nil || len(disks) != 1 || disks[0].Used != 255 || disks[0].Total != 1000 {
		t.Fatalf("invalid pool capacity %v %v", disks, err)
	}
	_, err = parseThinPools([]byte(`{"report":[{"lv":[{"segtype":"thin-pool","lv_size":"oops"}]}]}`), 100)
	if err == nil {
		t.Fatal("invalid measurement accepted")
	}
}
