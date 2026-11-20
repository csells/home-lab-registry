package main

import (
	"bufio"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestGuestStreamInitialChangeAndQuiet(t *testing.T) {
	s := &guestState{}
	s.set(GuestReport{Status: "available", Checked: 1, Expires: 300, Guests: []Guest{{ID: "1", State: "running"}}})
	server := httptest.NewServer(http.HandlerFunc(s.stream))
	defer server.Close()
	response, err := server.Client().Get(server.URL)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	reader := bufio.NewReader(response.Body)
	raw, err := reader.ReadBytes('\n')
	if err != nil {
		t.Fatal(err)
	}
	var r GuestReport
	if json.Unmarshal(raw, &r) != nil || r.Guests[0].State != "running" {
		t.Fatal(string(raw))
	}
	s.mu.Lock()
	wake := s.changed
	s.mu.Unlock()
	s.set(GuestReport{Status: "available", Checked: 2, Expires: 301, Guests: []Guest{{ID: "1", State: "running"}}})
	select {
	case <-wake:
		t.Fatal("timestamp-only update caused content event")
	default:
	}
	s.set(GuestReport{Status: "available", Checked: 3, Expires: 302, Guests: []Guest{{ID: "1", State: "stopped"}}})
	result := make(chan []byte, 1)
	go func() { line, _ := reader.ReadBytes('\n'); result <- line }()
	select {
	case line := <-result:
		if json.Unmarshal(line, &r) != nil || r.Guests[0].State != "stopped" {
			t.Fatal(string(line))
		}
	case <-time.After(time.Second):
		t.Fatal("change not pushed")
	}
}
