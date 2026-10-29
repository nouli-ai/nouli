#!/usr/bin/env python3
"""Run real CLI tutorial payloads using owned loopback fixtures and a fresh home."""
import argparse
import http.server
import json
from pathlib import Path
import subprocess
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[1]
TUTORIAL = ROOT / 'fixtures/tutorial'


class Fixture(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers['Content-Length'])
        if not 0 <= length <= 16 * 1024 * 1024:
            raise ValueError('fixture request too large')
        body = json.loads(self.rfile.read(length))
        self.server.calls.append(body)
        questions = body['questions']
        if 'NODE:high' in body['state'] and self.server.fail_high:
            self.server.fail_high = False
            status, response = 503, b'{}'
        else:
            status = 200
            if 'travel' in questions:
                path = ROOT / 'fixtures/providers/typesafe/response.json'
            elif questions['quality']['criteria'][0] == 'Over budget':
                path = ROOT / 'fixtures/journeys/typesafe-budget-response.json'
            else:
                path = ROOT / 'fixtures/journeys/typesafe-food-response.json'
            response = path.read_bytes()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(response)))
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, *_args):
        pass


def exercise(binary, home, server):
    store = home / 'store'
    config = home / 'config.toml'
    config.write_text(
        'default_provider="local"\ndefault_model="fixture-requested-alias"\noutput="json"\n[execution]\n'
        'max_attempts=1\nattempt_timeout_seconds=5\n[providers.local]\n'
        f'protocol="typesafe"\nbase_url="http://127.0.0.1:{server.server_port}"\n'
        '[providers.local.models."fixture-requested-alias"]\ncontext_budget_tokens=100000\n'
        'reserved_output_tokens=128\n'
    )
    environment = {'HOME': str(home)}

    def command(arguments, expected=0, stdin=None, raw=False):
        result = subprocess.run(
            [str(binary), '--config', str(config), '--store', str(store), *arguments],
            input=stdin, capture_output=True, env=environment, timeout=20,
        )
        if result.returncode != expected or result.stderr:
            raise AssertionError(f'command {arguments}: exit={result.returncode}, stderr={result.stderr!r}')
        return result.stdout if raw else json.loads(result.stdout)

    # Help must work with a missing HOME/config/store and must not create a store.
    for arguments in [[], ['--help'], ['--version'], ['workflows', '--help'],
                      ['workflows', 'run', '--help'], ['workflows', 'show', '--help'],
                      ['workflows', 'resume', '--help'], ['sessions', '--help'],
                      ['sessions', 'list', '--help'], ['sessions', 'show', '--help'],
                      ['sessions', 'export', '--help'], ['context', '--help']]:
        result = subprocess.run([str(binary), *arguments], env={}, capture_output=True, timeout=10)
        assert result.returncode == 0 and not result.stderr
    assert not store.exists() and not (home / '.nouli').exists()

    def turn(arguments, **kwargs):
        envelope = command(arguments, **kwargs)
        assert envelope['version'] == 1 and envelope['result']['kind'] == 'turn'
        return envelope['result']['report']

    meal_args = ['--input', str(TUTORIAL / 'meal.json'), '--name', 'Dinner', '--request-id', 'tutorial-meal']
    meal = turn(meal_args)
    calls = len(server.calls)
    replay = turn(['--input', str(TUTORIAL / 'meal.toon'), '--name', 'Dinner', '--request-id', 'tutorial-meal'])
    assert replay['replayed'] and replay['turn'] == meal['turn'] and len(server.calls) == calls
    budget = turn(['--input', str(TUTORIAL / 'commitment.json'), '--session', 'Dinner'])
    branch = turn(['--input', str(TUTORIAL / 'meal.json'), '--from', 'Dinner', '--name', 'Branch'])
    blend = turn(['--input', str(TUTORIAL / 'meal.json'), '--from', 'Dinner', '--from', 'Branch', '--name', 'Blend'])
    turn(['--input', str(TUTORIAL / 'supersede.json'), '--session', 'Dinner'])
    turn(['--input', str(TUTORIAL / 'unpin.json'), '--session', 'Branch'])
    continued = turn(['--input', '-', '--input-format', 'json', '--session', 'Blend'], stdin=(TUTORIAL / 'meal.json').read_bytes())
    context = command(['context', 'Blend', '--turn', continued['turn']])
    assert '019a0000-0000-7000-8000-000000000011' not in context['turn']['manifest']['context_toon']
    exported = command(['sessions', 'export', 'Blend'])
    assert len(exported['turns']) == 5
    assert len(command(['sessions', 'list'])['sessions']) == 3
    command(['sessions', 'show', 'Branch', '--format', 'text'], raw=True)
    command(['sessions', 'export', 'Blend', '--format', 'toon'], raw=True)
    before = (store / 'ledger.sqlite3').read_bytes()
    ephemeral = turn(['--input', str(TUTORIAL / 'meal.json'), '--from', 'Blend', '--no-session'])
    assert ephemeral['session'] is None and ephemeral['turn'] is None
    assert before == (store / 'ledger.sqlite3').read_bytes()
    turn(['-s', 'A short travel decision', '-q', json.dumps(json.loads((ROOT / 'fixtures/contracts/decision-request.json').read_text())['questions'][:3]), '--no-session'])
    report = command(['workflows', 'run', str(TUTORIAL / 'workflow.json'), '--name', 'Travel'], expected=4)['result']['report']
    assert report['status'] == 'partial' and len(report['nodes']) == 5
    run = report['run']
    command(['workflows', 'show', run])
    calls = len(server.calls)
    events = [json.loads(line) for line in command(['workflows', 'resume', run, '--jsonl'], raw=True).splitlines()]
    assert [event['sequence'] for event in events] == list(range(len(events)))
    assert all(event['version'] == 1 and event['run'] == run for event in events)
    assert sum(event['event']['type'] == 'terminal' for event in events) == 1
    done = events[-1]['event']['result']['result']['report']
    assert done['status'] == 'completed' and done['nodes'][0]['turn'] == report['nodes'][0]['turn']
    assert len(server.calls) - calls == 3
    assert all(node['status'] != 'running' for node in done['nodes'])
    assert not (home / '.nouli').exists()
    return {'provider_calls': len(server.calls), 'workflow_nodes': len(done['nodes']),
            'saved_initial': report['nodes'][0]['turn'] == done['nodes'][0]['turn'],
            'ephemeral_unchanged': True, 'status': 'passed'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/nouli')
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    server = http.server.HTTPServer(('127.0.0.1', 0), Fixture)
    server.calls = []
    server.fail_high = True
    thread = threading.Thread(target=server.serve_forever)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix='nouli-walkthrough-') as directory:
            result = exercise(binary, Path(directory), server)
            print(json.dumps(result, sort_keys=True))
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == '__main__':
    main()
