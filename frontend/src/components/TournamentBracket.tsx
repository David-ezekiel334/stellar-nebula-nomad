import React from 'react';

/** Mirrors `tournament::TournamentMatch` (Issue #587). */
export type MatchStatus = 'pending' | 'ready' | 'live' | 'done' | 'skipped';
export type BracketSide = 'winners' | 'losers' | 'grand_final' | 'reset' | 'pool';

export interface BracketMatchView {
  matchId: number;
  round: number;
  side: BracketSide;
  player1?: string;
  player2?: string;
  winner?: string;
  status: MatchStatus;
  /** UTC seconds. */
  scheduledAt?: number;
  deadline?: number;
  disputed?: boolean;
}

export interface TournamentBracketProps {
  matches: BracketMatchView[];
  /** Organizer display offset in minutes (e.g. 60 for UTC+1). */
  timezoneOffsetMinutes?: number;
  onSelectMatch?: (matchId: number) => void;
}

const SIDE_LABEL: Record<BracketSide, string> = {
  winners: 'Winners bracket',
  losers: 'Losers bracket',
  grand_final: 'Grand final',
  reset: 'Grand final reset',
  pool: 'Rounds',
};

const short = (addr?: string) => (addr ? `${addr.slice(0, 4)}…${addr.slice(-4)}` : 'BYE');

export function formatKickoff(utcSeconds: number, offsetMinutes = 0): string {
  const local = new Date((utcSeconds + offsetMinutes * 60) * 1000);
  const sign = offsetMinutes >= 0 ? '+' : '-';
  const abs = Math.abs(offsetMinutes);
  const tz = `UTC${sign}${String(Math.floor(abs / 60)).padStart(2, '0')}:${String(abs % 60).padStart(2, '0')}`;
  return `${local.toISOString().slice(0, 16).replace('T', ' ')} ${tz}`;
}

const MatchCard: React.FC<{
  m: BracketMatchView;
  tz: number;
  onSelect?: (id: number) => void;
}> = ({ m, tz, onSelect }) => {
  const row = (p?: string) => (
    <div
      style={{
        display: 'flex',
        justifyContent: 'space-between',
        fontWeight: m.winner && m.winner === p ? 700 : 400,
        opacity: m.status === 'done' && m.winner !== p ? 0.6 : 1,
      }}
    >
      <span>{short(p)}</span>
      {m.winner && m.winner === p && <span aria-label="winner">✓</span>}
    </div>
  );
  return (
    <button
      type="button"
      onClick={() => onSelect?.(m.matchId)}
      data-testid={`match-${m.matchId}`}
      style={{
        textAlign: 'left',
        background: '#0F172A',
        color: '#E2E8F0',
        border: `1px solid ${m.disputed ? '#F59E0B' : m.status === 'live' ? '#38BDF8' : '#334155'}`,
        borderRadius: '8px',
        padding: '8px 10px',
        minWidth: '170px',
        cursor: onSelect ? 'pointer' : 'default',
        opacity: m.status === 'skipped' ? 0.4 : 1,
      }}
    >
      {row(m.player1)}
      {row(m.player2)}
      <small style={{ color: '#94A3B8' }}>
        #{m.matchId} · {m.status}
        {m.disputed ? ' · disputed' : ''}
        {m.scheduledAt ? ` · ${formatKickoff(m.scheduledAt, tz)}` : ''}
      </small>
    </button>
  );
};

/** Column-per-round bracket, grouped by bracket side. */
export const TournamentBracket: React.FC<TournamentBracketProps> = ({
  matches,
  timezoneOffsetMinutes = 0,
  onSelectMatch,
}) => {
  const sides = (Object.keys(SIDE_LABEL) as BracketSide[]).filter((s) =>
    matches.some((m) => m.side === s),
  );
  if (matches.length === 0) {
    return <p style={{ color: '#94A3B8' }}>Bracket is generated when the tournament starts.</p>;
  }
  return (
    <div style={{ display: 'grid', gap: '20px' }}>
      {sides.map((side) => {
        const inSide = matches.filter((m) => m.side === side);
        const rounds = Array.from(new Set(inSide.map((m) => m.round))).sort((a, b) => a - b);
        return (
          <section key={side}>
            <h3 style={{ margin: '0 0 8px', color: '#E2E8F0' }}>{SIDE_LABEL[side]}</h3>
            <div style={{ display: 'flex', gap: '16px', overflowX: 'auto' }}>
              {rounds.map((r) => (
                <div key={r} style={{ display: 'grid', gap: '10px', alignContent: 'space-around' }}>
                  <strong style={{ color: '#94A3B8' }}>Round {r}</strong>
                  {inSide
                    .filter((m) => m.round === r)
                    .map((m) => (
                      <MatchCard key={m.matchId} m={m} tz={timezoneOffsetMinutes} onSelect={onSelectMatch} />
                    ))}
                </div>
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
};

export default TournamentBracket;
