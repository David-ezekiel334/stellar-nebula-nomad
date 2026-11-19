import React, { useMemo, useState } from 'react';
import { BracketMatchView, TournamentBracket, formatKickoff } from '../components/TournamentBracket';

/** Mirrors `tournament::TournamentFormat`. */
export type TournamentFormat = 'single_elimination' | 'double_elimination' | 'round_robin' | 'swiss';
export type TournamentStatus = 'reg' | 'active' | 'final' | 'done' | 'cancelled';

export interface StandingView {
  player: string;
  wins: number;
  losses: number;
  tiebreak: number;
  disqualified?: boolean;
}

export interface TournamentView {
  id: number;
  name: string;
  format: TournamentFormat;
  status: TournamentStatus;
  organizer: string;
  entryFee: number;
  resource: string;
  prizePool: number;
  prizeDistributionBps: number[];
  registrants: number;
  maxPlayers: number;
  registrationDeadline: number;
  timezoneOffsetMinutes: number;
  openDisputes: number;
  matches: BracketMatchView[];
  standings: StandingView[];
}

/** Contract calls wired by the host app (see docs/TOURNAMENT_GUIDE.md). */
export interface TournamentActions {
  register?: (id: number) => void;
  checkIn?: (id: number, matchId: number) => void;
  claimForfeit?: (id: number, matchId: number) => void;
  raiseDispute?: (id: number, matchId: number) => void;
  // Admin panel
  start?: (id: number) => void;
  cancel?: (id: number) => void;
  setResult?: (id: number, matchId: number, winner: string) => void;
  resolveDispute?: (id: number, matchId: number, overturn: boolean) => void;
  finalize?: (id: number) => void;
}

export interface TournamentsProps {
  tournaments: TournamentView[];
  viewer?: string;
  actions?: TournamentActions;
}

const FORMAT_LABEL: Record<TournamentFormat, string> = {
  single_elimination: 'Single elimination',
  double_elimination: 'Double elimination',
  round_robin: 'Round robin',
  swiss: 'Swiss',
};

const card: React.CSSProperties = {
  background: '#0F172A',
  border: '1px solid #334155',
  borderRadius: '8px',
  padding: '14px',
};

const btn: React.CSSProperties = {
  background: '#1E293B',
  color: '#E2E8F0',
  border: '1px solid #475569',
  borderRadius: '6px',
  padding: '6px 10px',
  cursor: 'pointer',
};

export const Tournaments: React.FC<TournamentsProps> = ({ tournaments, viewer, actions = {} }) => {
  const [selectedId, setSelectedId] = useState<number | undefined>(tournaments[0]?.id);
  const [selectedMatch, setSelectedMatch] = useState<number | undefined>();
  const t = useMemo(() => tournaments.find((x) => x.id === selectedId), [tournaments, selectedId]);
  const isAdmin = !!t && !!viewer && viewer === t.organizer;
  const match = t?.matches.find((m) => m.matchId === selectedMatch);
  const inMatch = !!match && !!viewer && (match.player1 === viewer || match.player2 === viewer);

  return (
    <section style={{ color: '#E2E8F0', display: 'grid', gap: '16px' }}>
      <h2 style={{ margin: 0 }}>Tournaments</h2>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(220px, 1fr))', gap: '12px' }}>
        {tournaments.map((x) => (
          <button
            key={x.id}
            type="button"
            onClick={() => setSelectedId(x.id)}
            style={{ ...card, textAlign: 'left', color: '#E2E8F0', outline: x.id === selectedId ? '2px solid #38BDF8' : 'none' }}
          >
            <strong>{x.name}</strong>
            <p style={{ margin: '4px 0' }}>{FORMAT_LABEL[x.format]} · {x.status}</p>
            <small>
              {x.registrants}/{x.maxPlayers} players · pool {x.prizePool} {x.resource}
            </small>
          </button>
        ))}
      </div>

      {t && (
        <>
          <div style={card}>
            <h3 style={{ marginTop: 0 }}>{t.name}</h3>
            <p>
              Entry {t.entryFee} {t.resource} · payouts{' '}
              {t.prizeDistributionBps.map((b) => `${b / 100}%`).join(' / ')}
            </p>
            {t.status === 'reg' && (
              <p>
                Registration closes {formatKickoff(t.registrationDeadline, t.timezoneOffsetMinutes)}{' '}
                <button type="button" style={btn} onClick={() => actions.register?.(t.id)}>
                  Register
                </button>
              </p>
            )}
            {t.status === 'final' && (
              <p>Results are in; the dispute window is open before prizes are paid.</p>
            )}
          </div>

          <TournamentBracket
            matches={t.matches}
            timezoneOffsetMinutes={t.timezoneOffsetMinutes}
            onSelectMatch={setSelectedMatch}
          />

          {match && (
            <div style={card} data-testid="match-panel">
              <strong>Match #{match.matchId}</strong> ({match.status})
              <div style={{ display: 'flex', gap: '8px', marginTop: '8px', flexWrap: 'wrap' }}>
                {inMatch && match.status === 'ready' && (
                  <>
                    <button type="button" style={btn} onClick={() => actions.checkIn?.(t.id, match.matchId)}>
                      Check in
                    </button>
                    <button type="button" style={btn} onClick={() => actions.claimForfeit?.(t.id, match.matchId)}>
                      Claim forfeit
                    </button>
                  </>
                )}
                {inMatch && match.status === 'done' && (
                  <button type="button" style={btn} onClick={() => actions.raiseDispute?.(t.id, match.matchId)}>
                    Dispute result
                  </button>
                )}
                {isAdmin && (match.status === 'ready' || match.status === 'live') &&
                  [match.player1, match.player2].filter(Boolean).map((p) => (
                    <button key={p} type="button" style={btn} onClick={() => actions.setResult?.(t.id, match.matchId, p!)}>
                      Award to {p!.slice(0, 6)}
                    </button>
                  ))}
                {isAdmin && match.disputed && (
                  <>
                    <button type="button" style={btn} onClick={() => actions.resolveDispute?.(t.id, match.matchId, true)}>
                      Overturn
                    </button>
                    <button type="button" style={btn} onClick={() => actions.resolveDispute?.(t.id, match.matchId, false)}>
                      Uphold
                    </button>
                  </>
                )}
              </div>
            </div>
          )}

          {t.standings.length > 0 && (
            <table style={{ ...card, borderCollapse: 'collapse' }}>
              <thead>
                <tr>
                  <th align="left">#</th>
                  <th align="left">Player</th>
                  <th>W</th>
                  <th>L</th>
                  <th>TB</th>
                </tr>
              </thead>
              <tbody>
                {t.standings.map((s, i) => (
                  <tr key={s.player} style={{ opacity: s.disqualified ? 0.5 : 1 }}>
                    <td>{i + 1}</td>
                    <td>{s.player.slice(0, 8)}</td>
                    <td align="center">{s.wins}</td>
                    <td align="center">{s.losses}</td>
                    <td align="center">{s.tiebreak}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          {isAdmin && (
            <div style={card} data-testid="admin-panel">
              <strong>Admin panel</strong> · open disputes: {t.openDisputes}
              <div style={{ display: 'flex', gap: '8px', marginTop: '8px' }}>
                {t.status === 'reg' && (
                  <button type="button" style={btn} onClick={() => actions.start?.(t.id)}>
                    Start tournament
                  </button>
                )}
                {t.status === 'final' && (
                  <button type="button" style={btn} onClick={() => actions.finalize?.(t.id)}>
                    Finalize &amp; pay prizes
                  </button>
                )}
                {t.status !== 'done' && t.status !== 'cancelled' && (
                  <button type="button" style={btn} onClick={() => actions.cancel?.(t.id)}>
                    Cancel &amp; refund
                  </button>
                )}
              </div>
            </div>
          )}
        </>
      )}
    </section>
  );
};

export default Tournaments;
