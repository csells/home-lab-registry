import { useEffect, useState } from 'react';
import type { RefObject } from 'react';
import type { Metrics } from './inventory.ts';
import { chart, metricNames, metricValue } from './telemetry.ts';
import type { MetricName } from './telemetry.ts';

function useMetrics(machine: string, card: RefObject<HTMLElement | null>, pageVisible: boolean) {
  const [visible, setVisible] = useState(false);
  const [reading, setReading] = useState<Metrics | null>(null);

  useEffect(() => {
    const element = card.current;
    if (!element) return;
    const observer = new IntersectionObserver(entries => {
      setVisible(entries.some(entry => entry.isIntersecting && entry.intersectionRatio > 0));
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [card]);

  useEffect(() => {
    if (!pageVisible || !visible) return;
    const source = new EventSource('api/metrics/stream?machine=' + encodeURIComponent(machine));
    let active = true;
    source.addEventListener('metrics', event => {
      if (!active) return;
      try {
        const snapshot: Metrics = JSON.parse(event.data);
        if (!snapshot || typeof snapshot.status !== 'string') throw new Error('Invalid metrics');
        setReading(snapshot);
      } catch {
        setReading(previous => ({ ...previous, status: 'unavailable' }));
      }
    });
    source.onerror = () => {
      if (active) setReading(previous => ({ ...previous, status: 'unavailable' }));
    };
    return () => {
      active = false;
      source.close();
    };
  }, [machine, pageVisible, visible]);
  return reading;
}

function Plot({ name, reading, now, fresh }: { name: MetricName; reading: Metrics; now: number; fresh: boolean }) {
  const series = chart(reading.history ?? [], name, now);
  const current = fresh ? metricValue(reading, name) : null;
  const gpu = name === 'GPU' ? (fresh ? reading.gpu?.status : undefined) : null;
  const overlay = name === 'GPU' && gpu !== 'available';
  const absent = gpu === 'not_applicable';
  const range = overlay ? absent ? 'No GPU' : 'No reading' : series.scale + ' scale';
  let title: string | undefined;
  if (name === 'GPU') title = reading.gpu?.note || 'GPU monitoring requires an updated collector';
  if (name === 'DISK') {
    title = 'Most-used storage pool · ' + (reading.disks ?? []).filter(disk => disk.total > 0)
      .map(disk => disk.name + ': ' + (100 * disk.used / disk.total).toFixed(1) + '%').join(' · ');
  }
  return <div className={'telemetry-cell ' + name.toLowerCase()} title={title}>
    <div className="telemetry-label"><span>{name}</span><span className="telemetry-value">
      {current === null ? '—' : current.toFixed(1) + '%'}
    </span></div>
    <div className="telemetry-plot">
      <svg viewBox="0 0 240 54" preserveAspectRatio="none" className="trend" role="img"
        aria-hidden={overlay || undefined} data-range={series.scale}
        aria-label={name + ' usage history: last 10 minutes, scale ' + series.low + ' to ' + series.high + ' percent'}>
        {series.bars.map((bar, index) => <rect key={index} x={index * 8} y={53 - bar.height} width="5" rx="1" height={bar.height}
          className={bar.sample ? 'history-bar' : 'history-empty'}>
          <title>{bar.sample && bar.value !== undefined && bar.sample.checked !== undefined
            ? bar.value.toFixed(1) + '% · ' + new Date(bar.sample.checked * 1000).toLocaleTimeString() : 'No sample'}</title>
        </rect>)}
      </svg>
      {overlay && <div className="telemetry-overlay" role="status"
        aria-label={absent ? 'GPU: no accelerator available' : 'GPU: utilization unavailable'}>{absent ? 'N/A' : 'Unavailable'}</div>}
    </div>
    <div className="telemetry-range">{range}</div>
  </div>;
}

export function Telemetry({ machine, host, card, cached, pageVisible, now }: {
  machine: string; host: string; card: RefObject<HTMLElement | null>; cached: Metrics; pageVisible: boolean; now: number;
}) {
  const liveReading = useMetrics(machine, card, pageVisible);
  const reading = liveReading ?? cached;
  const fresh = reading.status === 'available' && (!reading.expires || reading.expires > now);
  const live = !!liveReading && fresh && pageVisible;
  const checked = reading.checked ? new Date(reading.checked * 1000).toLocaleTimeString([], {
    hour: '2-digit', minute: '2-digit', second: '2-digit',
  }) : 'Just now';
  const status = live ? 'Live · ' + checked : fresh ? 'Cached history' : reading.status === 'waiting'
    ? 'Connecting stream' : 'Readings unavailable';
  return <section className="metrics-panel telemetry-strip" aria-label={host + ' resource history'}>
    <div className="telemetry-charts">
      {metricNames.map(name => <Plot key={name} name={name} reading={reading} now={now} fresh={fresh} />)}
    </div>
    <div className="telemetry-caption" title={reading.warnings?.join('; ')}>
      <span className={'telemetry-state' + (live ? ' is-live' : '')}>{status}{reading.warnings?.length ? ' · warning' : ''}</span>
      <span>10 min → now</span>
    </div>
  </section>;
}
