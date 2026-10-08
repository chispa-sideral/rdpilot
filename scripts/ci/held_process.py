"""Register a concrete owned child; failed registration cannot orphan it."""
import asyncio
import json
import time


def _register(child, identity, journal, records):
    record = identity(child.pid)
    records.append(record)
    journal.write_text(json.dumps(records))
    return record


def abort_sync(child, timeout=15):
    """Always attempt both kill and join, sharing one finite join budget."""
    deadline = time.monotonic() + timeout
    failures = []
    try:
        child.kill()
    except BaseException:
        failures.append('kill')
    try:
        child.wait(timeout=max(0, deadline - time.monotonic()))
    except BaseException:
        failures.append('join')
    return tuple(failures)


async def abort_async(child, timeout=5):
    deadline = time.monotonic() + timeout
    failures = []
    try:
        child.kill()
    except BaseException:
        failures.append('kill')
    try:
        await asyncio.wait_for(child.wait(), max(0,deadline-time.monotonic()))
    except BaseException:
        failures.append('join')
    return tuple(failures)


def register_sync(child, identity, journal, records, timeout=15):
    try:
        return _register(child, identity, journal, records)
    except BaseException as error:
        error.held_cleanup_failures = abort_sync(child, timeout)
        raise


async def register_async(child, identity, journal, records, timeout=5):
    try:
        return _register(child, identity, journal, records)
    except BaseException as error:
        error.held_cleanup_failures = await abort_async(child, timeout)
        raise
