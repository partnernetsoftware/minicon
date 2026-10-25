#!/usr/bin/env python3
"""Same-image direct-wry lifetime comparison; diagnostic only, owner PRD28.01.

Both arms keep the original fixture/page/eight predicates. This does not invoke
ConApp, change normal qualification, call providers or claim persistence repair.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import shutil
import sys
import tomllib
import time
import fixture
import run as native
import qualify as qualification

ROOT=Path(__file__).resolve().parents[2]
# Conservative local dependency superset, not a claim that all product paths
# were compiled/executed. External package versions/checksums remain lock-bound.
INPUTS=tuple(dict.fromkeys((*qualification.INPUTS,
    ".cargo/config.toml", "build.rs", "scripts/cleanup-build-state.py",
    "crates/minicon-platform/Cargo.toml", "crates/minicon-platform/build.rs", "crates/minicon-core/Cargo.toml",
    "crates/minicon-web/tests/support/native_auth_fixture.rs",
    "crates/minicon-web/examples/windows-cookie-lifetime.rs",
    "crates/minicon-web/examples/windows_cookie_lifetime/host.rs",
    "crates/minicon-web/examples/windows_cookie_lifetime/cookie_store_histograms.rs",
    "scripts/native-auth-context/lifetime.py",
    "scripts/native-auth-context/lifetime_metadata_selfcheck.py",
    *(p.relative_to(ROOT).as_posix() for p in sorted((ROOT/"crates/minicon-web/src").rglob("*.rs"))),
    *(p.relative_to(ROOT).as_posix() for p in sorted((ROOT/"crates/minicon-platform/src").rglob("*.rs"))),
    *(p.relative_to(ROOT).as_posix() for p in sorted((ROOT/"crates/minicon-core/src").rglob("*.rs"))),
)))


def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def write(p,v):fixture.private_json(p,v)
def read(p):return native.read_json(p)
def source():return {p:sha(ROOT/p) for p in INPUTS}
def pe_x64(p):
    with p.open("rb") as stream:
        head=stream.read(64)
        if len(head)!=64 or head[:2]!=b"MZ":return False
        offset=struct.unpack_from("<I",head,60)[0]
        if offset>16*1024*1024:return False
        stream.seek(offset);return stream.read(6)==b"PE\0\0d\x86"

class BeforeReleaseFiles:
    """One exact pending B handoff; filesystem work uses the existing HTTP worker."""
    def __init__(self, state, manifest, output):
        self.state, self.manifest, self.output = state, manifest, output
        self.used = False

    def observe(self, body):
        unavailable = {"status": "unavailable"}
        try:
            with self.state.lock:
                if time.monotonic() >= self.state.deadline:
                    return unavailable
                expected = self.state.status()["action"]
                if (self.used or set(body) != {"run", "action", "locator"}
                        or body["run"] != self.state.run
                        or expected is None or expected["seq"] != 2
                        or expected["phase"] != "b" or expected["kind"] != "present-auth"
                        or body["action"] != expected or 1 not in self.state.acked):
                    return unavailable
                self.used = True
            deadline = min(self.state.deadline, time.monotonic() + 1.5)
            result = native.before_release_file_metadata(self.manifest, self.output, body["locator"], deadline)
            with self.state.lock:
                if time.monotonic() >= deadline or self.state.status()["action"] != expected:
                    return unavailable
            return result
        except (OSError, ValueError, KeyError, TypeError, fixture.Rejected):
            return unavailable


class LifetimeMetadataHandler(fixture.Handler):
    def route(self):
        if self.path != "/driver/lifetime-before-release":
            return super().route()
        self.driver()
        if self.command != "POST":
            raise fixture.Rejected(405, "method-rejected")
        # At most one admitted diagnostic; do not change original request-count
        # or deadline failure state merely to observe optional file metadata.
        observation = self.server.before_release_files.observe(self.body())
        self.respond(200, {"run": self.server.state.run, "seq": 2,
                           "phase": "a-before-context-release", "observation": observation})


def arm(binary,mode,output,timeout,identity,expected_source):
    output.mkdir(mode=0o700,exist_ok=False)
    receipt={"kind":"direct-wry-lifetime-arm","test_only":True,"mode":mode,
             "native_qualification":False,"production_repair":False,"status":"BLOCKED",
             "binary_sha256":identity,"source_sha256":expected_source,"native_execution_started":False,
             "original_fixture_status":"NOT_OBSERVED","original_eight":{},"failure_code":None}
    owned=None;manifest=None;court=None;diagnostics=[];clock=(time.time(),time.monotonic())
    try:
        if sha(binary)!=identity or source()!=expected_source:raise native.CourtError("lifetime-source-image-changed")
        with fixture.Fixture(timeout) as court:
            manifest=court.manifest(output);write(output/"manifest.private.json",manifest)
            court.server.before_release_files=BeforeReleaseFiles(court.state,manifest,output)
            court.server.RequestHandlerClass=LifetimeMetadataHandler
            env=dict(os.environ)
            for name in manifest["clear_environment"]:env.pop(name,None)
            env.update(manifest["launch_environment"])
            owned=native.OwnedProcess([str(binary),str(output/"manifest.private.json"),mode],env,output)
            receipt["native_execution_started"]=True
            while not owned.exited():
                if time.monotonic()>=court.state.deadline:raise native.CourtError("lifetime-original-deadline-expired")
                if any((output/p).stat().st_size>native.LIMIT for p in ("driver.stdout.log","driver.stderr.log")):
                    raise native.CourtError("lifetime-driver-output-bound")
                time.sleep(0.05)  # Existing owned-process polling; no persistence delay/retry.
            receipt["actual_exit_code"]=owned.process.wait(timeout=1)
            diagnostics.append(native.windows_cookie_snapshot(manifest,output,"driver-exited-before-job-cleanup",clock))
            driver=read(output/"native-driver-receipt.json")
            if (driver.get("kind")!="windows-cookie-lifetime-direct-wry-diagnostic"
                    or driver.get("test_only") is not True or driver.get("mode")!=mode
                    or driver.get("native_qualification") is not False or driver.get("production_repair") is not False
                    or driver.get("wry_package")!="0.56.1"
                    or driver.get("diagnostic_features")!=["minicon-web/native-profile-diagnostics"]
                    or driver.get("profile_acl_mode")!="owner-only"):
                raise native.CourtError("lifetime-driver-identity-invalid")
            observed=court.state.receipt()
            write(output/"fixture-receipt.json",observed)
            receipt["original_fixture_status"]=observed["observation_status"]
            receipt["original_eight"]={k:observed["checks"].get(k) for k in native.CHECKS}
            receipt["original_failure_code"]=observed["failure"]
            receipt["driver"]=driver
            valid_original=False
            if observed["observation_status"]=="complete" and observed["failure"] is None:
                native.validate_original_fixture(driver["fixture"], observed, court.state.driver_receipt_snapshot)
                valid_original=driver["original_eight_complete"] is True
            if not driver["boundary_valid"]:
                receipt.update(status="BLOCKED",failure_code=driver["boundary_failure_code"] or "lifetime-boundary-unavailable")
            elif valid_original and receipt["actual_exit_code"]==0:
                receipt["status"]="PASS"
            elif observed["failure"] is not None:
                receipt.update(status="FAIL",failure_code=observed["failure"])
            else:
                receipt.update(status="BLOCKED",failure_code="lifetime-driver-nonzero-without-oracle-failure" if receipt["actual_exit_code"] else "lifetime-original-observation-incomplete")
    except native.CourtError as error:receipt.update(status="BLOCKED",failure_code=error.code)
    except (OSError,ValueError,KeyError,TypeError):receipt.update(status="BLOCKED",failure_code="lifetime-driver-or-public-receipt-unavailable")
    finally:
        if owned is not None:
            try:owned.close()
            except native.CourtError:receipt.update(status="BLOCKED",failure_code="lifetime-owned-cleanup-unavailable")
            receipt["cleanup"]=owned.cleanup
        if court is not None:
            f=court.state.receipt();write(output/"fixture-receipt.json",f)
            receipt["original_eight"]={k:f["checks"].get(k) for k in native.CHECKS}
            receipt["original_fixture_status"]=f["observation_status"]
            receipt["original_oracle_verdict"]="FAIL" if f["failure"] is not None else "PASS" if all(f["checks"].get(k) is True for k in native.CHECKS) else "NOT_OBSERVED"
            receipt["original_failure_code"]=f["failure"]
        if manifest is not None:
            diagnostics.append(native.windows_cookie_snapshot(manifest,output,"after-owned-job-cleanup",clock))
        write(output/"windows-cookie-diagnostics.json",{"kind":"owned-fresh-windows-cookie-diagnostics","status":"diagnostics-only","changes_court_verdict":False,"snapshots":diagnostics,
              "limitations":["Only exact fresh manifest A/B and verified ProfilePath fixed slots; no discovery/decryption/WAL writes.","Post-exit observations cannot reconstruct pre-close disk contents; normal exit is not a flush acknowledgment.","One extra pre-close causal HTTP handoff may affect timing; accepted metadata is not Cookie contents or a persistence fix."]})
        receipt["source_unchanged"]=source()==expected_source;receipt["binary_unchanged"]=sha(binary)==identity
        if not receipt["source_unchanged"] or not receipt["binary_unchanged"]:receipt.update(status="BLOCKED",failure_code="lifetime-source-image-changed")
        receipt["artifacts"]={name:{"sha256":sha(output/name),"bytes":(output/name).stat().st_size} for name in
                              ("native-driver-receipt.json","fixture-receipt.json","windows-cookie-diagnostics.json","driver.stdout.log","driver.stderr.log") if (output/name).is_file()}
        write(output/"receipt.json",receipt)
    return receipt

def build_and_run(args):
    """One opt-in dedicated test build, then the unchanged two-arm runner."""
    output=args.output.resolve()
    output.mkdir(mode=0o700,parents=True,exist_ok=False)
    report={"kind":"direct-wry-lifetime-build-driver","status":"BLOCKED",
            "test_only":True,"native_qualification":False,"release_gate":False,
            "production_repair":False,"commands":[],"comparison_started":False,
            "source_scope":"qualify.INPUTS plus local web/platform/core Rust dependency superset; not whole-product execution",
            "failure_code":None}
    canonical=None;canonical_hash=None;original=None;exit_code=2
    def run_step(label,argv,timeout):
        log=output/(label+".log")
        code=qualification.command(argv,log,env,timeout)
        report["commands"].append({"label":label,"argv":argv,"exit_code":code,"log_sha256":sha(log)})
        if source()!=original or sha(canonical)!=canonical_hash:
            raise native.CourtError("lifetime-source-or-canonical-changed",True)
        return code,log.read_text(encoding="utf-8",errors="replace")
    try:
        canonical=native.executable(args.canonical_product)
        if not pe_x64(canonical):raise native.CourtError("lifetime-canonical-not-pe-x64",True)
        canonical_hash=sha(canonical);original=source()
        commit=subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT).decode("ascii").strip()
        if not re.fullmatch(r"[0-9a-f]{40}",commit):raise native.CourtError("lifetime-source-commit-invalid",True)
        report.update(source_commit=commit,source_sha256=original,canonical_product_sha256=canonical_hash)
        env=dict(os.environ,CARGO_TARGET_DIR=str(output/"cargo-target"),CARGO_TERM_COLOR="never")
        bash=qualification.choose_bash()
        code,_=run_step("protocol",[sys.executable,"-B",str(ROOT/"scripts/native-auth-context/verify_protocol.py"),"--output",str(output/"protocol")],180)
        if code:raise native.CourtError("lifetime-protocol-controls-failed")
        protocol_path=output/"protocol/protocol-guard-receipt.json";protocol=read(protocol_path)
        controls=protocol.get("expected_assertion_reds")
        if (protocol.get("byte_exact_source_restored") is not True or protocol.get("restored_returncode")!=0
                or type(protocol.get("restored_checks")) is not int or protocol["restored_checks"]<=0
                or not isinstance(controls,list) or len(controls)!=11
                or not all(c.get("assertion_red") is True for c in controls)
                or protocol.get("fixture_sha256")!=original["scripts/native-auth-context/fixture.py"]):
            raise native.CourtError("lifetime-protocol-evidence-invalid",True)
        suffix=["-p","minicon-web","--no-default-features","--features","windows-cookie-lifetime-court",
                "--example","windows-cookie-lifetime","--target",args.target]
        code,text=run_step("strict-example-tests",[bash,str(ROOT/"scripts/build.sh"),"test-suite",*suffix,"host::","--","--test-threads=1"],900)
        expected={
            'host::tests::backend_histograms_only_admit_the_two_current_pending_phases',
            'host::tests::preclose_metadata_requires_one_persistent_future_sdk_cookie',
            'host::tests::preclose_metadata_rejects_late_wrong_run_and_nonfinite_classification',
            'host::tests::actual_load_ack_accepts_only_the_original_ordinary_history_query',
            'host::tests::keeper_and_pending_owned_exit_are_both_required_at_the_real_reopen_gate',
            'host::tests::matched_exit_is_insufficient_until_both_old_keeper_and_environment_are_released',
            'host::tests::only_the_two_finite_lifetime_modes_are_executable',
            'host::tests::actual_dispatch_authority_refuses_changed_url_phase_kind_and_sequence',
            'host::cookie_store_histograms::tests::numeric_backend_problem_reduces_without_cookie_or_error_strings',
            'host::cookie_store_histograms::tests::missing_empty_unknown_and_nonfinite_shapes_remain_unknown',
            'host::cookie_store_histograms::tests::response_prefix_is_bounded_before_json_allocation',
            'host::cookie_store_histograms::tests::numeric_bucket_count_and_range_cannot_escape_fixed_bound',
            'host::cookie_store_histograms::tests::deadline_and_cancel_reject_late_callback_generation',
            'host::cookie_store_histograms::tests::duplicate_callback_cannot_replace_the_first_finite_result',
            'host::cookies::tests::cookie_metadata_is_bounded_and_never_turns_failure_into_zero',
            'host::cookies::tests::cookie_query_state_refuses_late_or_cancelled_completion',
            'host::cookies::tests::cookie_query_never_enumerates_a_whole_profile',
            'host::release::tests::only_the_owned_normal_exit_releases_the_observation',
            'host::release::tests::unreadable_events_fail_without_native_diagnostic_payloads',
            'host::release::tests::observation_terminal_states_do_not_change_on_late_events',
        }
        names=re.findall(r"^test (host::[^ ]+) \.\.\. ok$",text,re.M)
        counts=re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",text,re.M)
        if code or counts!=[("ok",str(len(expected)),"0","0")] or set(names)!=expected or len(names)!=len(expected):
            raise native.CourtError("lifetime-example-tests-failed-or-zero")
        code,text=run_step("dedicated-build",[bash,str(ROOT/"scripts/build.sh"),"dev",*suffix,"--message-format=json-render-diagnostics"],900)
        if code:raise native.CourtError("lifetime-dedicated-build-failed")
        executable=output/"cargo-target"/args.target/"debug/examples/windows-cookie-lifetime.exe"
        artifacts=[]
        for line in text.splitlines():
            try:item=json.loads(line)
            except ValueError:continue
            if (isinstance(item,dict) and item.get("reason")=="compiler-artifact"
                    and item.get("target",{}).get("name")=="windows-cookie-lifetime"
                    and item.get("target",{}).get("kind")==["example"] and item.get("executable")):
                artifacts.append(item)
        if len(artifacts)!=1 or Path(artifacts[0]["executable"]).resolve()!=executable.resolve():
            raise native.CourtError("lifetime-dedicated-build-artifact-mismatch",True)
        actual_features=artifacts[0].get("features",[])
        if (not {"windows-cookie-lifetime-court","native-page-host","native-profile-diagnostics"}<=set(actual_features)
                or {"wry-host","default"}&set(actual_features)):
            raise native.CourtError("lifetime-build-feature-mismatch",True)
        locked=tomllib.loads((ROOT/"Cargo.lock").read_text(encoding="utf-8"))
        wry=[p for p in locked["package"] if p["name"]=="wry"]
        if len(wry)!=1 or wry[0]["version"]!="0.56.1" or not re.fullmatch(r"[0-9a-f]{64}",wry[0].get("checksum","")):
            raise native.CourtError("lifetime-wry-lock-mismatch",True)
        identity=sha(executable)
        if not pe_x64(executable):raise native.CourtError("lifetime-built-image-not-pe-x64",True)
        pinned=output/"lifetime-pinned.exe"
        with executable.open("rb") as source_stream,pinned.open("xb") as destination:
            shutil.copyfileobj(source_stream,destination)
        if sha(pinned)!=identity or sha(executable)!=identity:raise native.CourtError("lifetime-pin-copy-changed",True)
        build_path=output/"build-receipt.json"
        build={"source_commit":commit,"source_sha256":original,"binary_sha256":identity,
               "os":"windows","isa":"x86_64","test_only":True,"wry_package":"0.56.1",
               "wry_lock_checksum":wry[0]["checksum"],"features":["windows-cookie-lifetime-court"],
               "actual_package_features":actual_features,"actual_build_command":report["commands"][-1],
               "native_qualification":False,"production_repair":False}
        write(build_path,build)
        report.update(binary_sha256=identity,actual_package_features=actual_features,build_receipt_sha256=sha(build_path),protocol_receipt_sha256=sha(protocol_path))
        report["comparison_started"]=True
        code,_=run_step("comparison",[sys.executable,"-B",str(ROOT/"scripts/native-auth-context/lifetime.py"),
            "--binary",str(pinned),"--build-receipt",str(build_path),"--protocol-receipt",str(protocol_path),
            "--output",str(output/"comparison"),"--timeout",str(args.timeout)],2*args.timeout+40)
        comparison=read(output/"comparison/lifetime-comparison-receipt.json")
        if (comparison.get("source_commit")!=commit or comparison.get("binary_sha256")!=identity
                or len(comparison.get("arms",[]))!=2
                or [a.get("mode") for a in comparison["arms"]]!=["keep-browser","normal-exit"]
                or sha(pinned)!=identity):raise native.CourtError("lifetime-comparison-identity-invalid",True)
        report["comparison_receipt_sha256"]=sha(output/"comparison/lifetime-comparison-receipt.json")
        report["original_arm_outcomes"]=comparison["arms"]
        report["status"]=comparison["status"]
        if report["status"] not in ("PASS","FAIL","BLOCKED") or code!={"PASS":0,"FAIL":1,"BLOCKED":2}[report["status"]]:
            raise native.CourtError("lifetime-comparison-exit-mismatch",True)
        exit_code=code
    except native.CourtError as error:
        report.update(status="BLOCKED" if error.blocked else "FAIL",failure_code=error.code)
        exit_code=2 if error.blocked else 1
    except (OSError,ValueError,KeyError,TypeError,UnicodeError,subprocess.SubprocessError):
        report.update(status="BLOCKED",failure_code="lifetime-build-or-receipt-unavailable");exit_code=2
    finally:
        try:stable=original is not None and source()==original and sha(canonical)==canonical_hash
        except (OSError,TypeError):stable=False
        report["source_and_canonical_unchanged"]=stable
        if not stable:report.update(status="BLOCKED",failure_code="lifetime-source-or-canonical-unavailable");exit_code=2
        write(output/"build-driver-receipt.json",report)
    return exit_code


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build",action="store_true",help="Windows x64 diagnostic build+both arms; not qualification")
    parser.add_argument("--target",choices=("x86_64-pc-windows-msvc",),default="x86_64-pc-windows-msvc")
    parser.add_argument("--canonical-product",type=Path)
    parser.add_argument("--binary",type=Path)
    parser.add_argument("--build-receipt",type=Path)
    parser.add_argument("--output",required=True,type=Path)
    parser.add_argument("--protocol-receipt",type=Path)
    parser.add_argument("--timeout",default=120,type=int)
    args=parser.parse_args()
    if os.name!="nt" or not 0<args.timeout<=120:parser.error("requires actual Windows and original deadline <=120s")
    if args.build:
        if args.canonical_product is None or any((args.binary,args.build_receipt,args.protocol_receipt)):
            parser.error("--build requires --canonical-product and no prebuilt overrides")
        return build_and_run(args)
    if args.canonical_product is not None or not all((args.binary,args.build_receipt,args.protocol_receipt)):
        parser.error("prebuilt comparison requires binary/build/protocol receipts without canonical override")
    binary=args.binary.resolve(strict=True);build=read(args.build_receipt)
    protocol=read(args.protocol_receipt)
    if (protocol.get("byte_exact_source_restored") is not True or protocol.get("restored_returncode")!=0
            or type(protocol.get("restored_checks")) is not int or protocol["restored_checks"]<=0
            or not protocol.get("expected_assertion_reds")
            or protocol.get("fixture_sha256")!=sha(ROOT/"scripts/native-auth-context/fixture.py")):
        parser.error("matching restored original fixture protocol receipt required")
    try:
        current_commit=subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT).decode("ascii").strip()
    except (OSError,subprocess.SubprocessError,UnicodeError):
        parser.error("exact source commit unavailable")
    identity=sha(binary);expected=source()
    if (not pe_x64(binary) or build.get("binary_sha256")!=identity or build.get("source_sha256")!=expected
            or build.get("os")!="windows" or build.get("isa")!="x86_64"
            or build.get("test_only") is not True or build.get("wry_package")!="0.56.1"
            or build.get("features")!=["windows-cookie-lifetime-court"]
            or not re.fullmatch(r"[0-9a-f]{40}",build.get("source_commit",""))
            or build.get("source_commit")!=current_commit):parser.error("exact dedicated test build/PE/source receipt required")
    output=args.output.resolve();output.mkdir(mode=0o700,parents=True,exist_ok=False)
    aggregate={"kind":"direct-wry-lifetime-comparison","status":"BLOCKED","source_commit":build["source_commit"],"binary_sha256":identity,"build_receipt_sha256":sha(args.build_receipt),"protocol_receipt_sha256":sha(args.protocol_receipt),"native_qualification":False,"production_repair":False,"timing_interference_possible":True,"arms":[]}
    for mode in ("keep-browser","normal-exit"):
        r=arm(binary,mode,output/mode,args.timeout,identity,expected)
        aggregate["arms"].append({"mode":mode,"status":r["status"],"original_failure_code":r.get("original_failure_code"),"original_oracle_verdict":r.get("original_oracle_verdict","NOT_OBSERVED"),"original_eight":r["original_eight"],"receipt_sha256":sha(output/mode/"receipt.json")})
        write(output/"lifetime-comparison-receipt.json",aggregate)
    versions=[r.get("driver",{}).get("runtime_version") for r in (read(output/m/"receipt.json") for m in ("keep-browser","normal-exit"))]
    aggregate["runtime_versions_match"]=all(isinstance(v,str) and v for v in versions) and versions[0]==versions[1]
    aggregate["source_unchanged"]=source()==expected;aggregate["binary_unchanged"]=sha(binary)==identity
    aggregate["status"]=("BLOCKED" if not aggregate["runtime_versions_match"] or not aggregate["source_unchanged"] or not aggregate["binary_unchanged"] or any(a["status"]=="BLOCKED" for a in aggregate["arms"]) else "PASS" if all(a["status"]=="PASS" for a in aggregate["arms"]) else "FAIL")
    aggregate["interpretation"]="Lifetime diagnostic only; KEEP success is an in-memory observation, never a persistence repair. MiniCon production original court remains independently required."
    write(output/"lifetime-comparison-receipt.json",aggregate)
    return 0 if aggregate["status"]=="PASS" else 2 if aggregate["status"]=="BLOCKED" else 1

if __name__=="__main__":raise SystemExit(main())
