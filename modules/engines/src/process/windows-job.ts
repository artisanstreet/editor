import koffi from "koffi";

const job_object_extended_limit_information = 9;
const job_object_limit_kill_on_job_close = 0x0000_2000;
const process_set_quota = 0x0100;
const process_terminate = 0x0001;

function create_windows_api() {
	if (process.platform !== "win32") {
		throw new Error("Windows Job Objects are unavailable on this platform");
	}

	const kernel32 = koffi.load("kernel32.dll");
	koffi.pointer("ArtisanWindowsHandle", koffi.opaque());
	const BasicLimitInformation = koffi.struct("ArtisanJobBasicLimitInformation", {
		per_process_user_time_limit: "int64",
		per_job_user_time_limit: "int64",
		limit_flags: "uint32",
		minimum_working_set_size: "size_t",
		maximum_working_set_size: "size_t",
		active_process_limit: "uint32",
		affinity: "uintptr_t",
		priority_class: "uint32",
		scheduling_class: "uint32",
	});
	const IoCounters = koffi.struct("ArtisanJobIoCounters", {
		read_operation_count: "uint64",
		write_operation_count: "uint64",
		other_operation_count: "uint64",
		read_transfer_count: "uint64",
		write_transfer_count: "uint64",
		other_transfer_count: "uint64",
	});
	const ExtendedLimitInformation = koffi.struct("ArtisanJobExtendedLimitInformation", {
		basic_limit_information: BasicLimitInformation,
		io_info: IoCounters,
		process_memory_limit: "size_t",
		job_memory_limit: "size_t",
		peak_process_memory_used: "size_t",
		peak_job_memory_used: "size_t",
	});
	const create_job_object = kernel32.func(
		"ArtisanWindowsHandle __stdcall CreateJobObjectW(void *attributes, const char16_t *name)",
	);
	const set_information_job_object = kernel32.func(
		"int __stdcall SetInformationJobObject(ArtisanWindowsHandle job, int info_class, const ArtisanJobExtendedLimitInformation *info, uint32 length)",
	);
	const open_process = kernel32.func(
		"ArtisanWindowsHandle __stdcall OpenProcess(uint32 desired_access, int inherit_handle, uint32 process_id)",
	);
	const assign_process_to_job = kernel32.func(
		"int __stdcall AssignProcessToJobObject(ArtisanWindowsHandle job, ArtisanWindowsHandle process)",
	);
	const terminate_job_object = kernel32.func(
		"int __stdcall TerminateJobObject(ArtisanWindowsHandle job, uint32 exit_code)",
	);
	const close_handle = kernel32.func("int __stdcall CloseHandle(ArtisanWindowsHandle handle)");
	const get_last_error = kernel32.func("uint32 __stdcall GetLastError()");

	return {
		ExtendedLimitInformation,
		assign_process_to_job,
		close_handle,
		create_job_object,
		get_last_error,
		open_process,
		set_information_job_object,
		terminate_job_object,
	};
}

let windows_api: ReturnType<typeof create_windows_api> | undefined;

function get_windows_api() {
	windows_api ??= create_windows_api();

	return windows_api;
}

/** Owns one Windows Job Object and its assigned engine process tree. @since 0.4.0 */
export interface WindowsJob {
	readonly Close: () => void;
	readonly Terminate: (exit_code: number) => void;
}

/** Holds an unassigned Job Object and exact candidate process handle during IPC proof. @since 0.4.0 */
export interface WindowsJobCandidate {
	readonly Assign: () => WindowsJob;
	readonly Close: () => void;
}

function make_native_error(operation: string, get_last_error: () => number) {
	return new Error(`${operation} failed with Windows error ${get_last_error()}`);
}

function make_limit_information() {
	return {
		basic_limit_information: {
			per_process_user_time_limit: 0,
			per_job_user_time_limit: 0,
			limit_flags: job_object_limit_kill_on_job_close,
			minimum_working_set_size: 0,
			maximum_working_set_size: 0,
			active_process_limit: 0,
			affinity: 0,
			priority_class: 0,
			scheduling_class: 0,
		},
		io_info: {
			read_operation_count: 0,
			write_operation_count: 0,
			other_operation_count: 0,
			read_transfer_count: 0,
			write_transfer_count: 0,
			other_transfer_count: 0,
		},
		process_memory_limit: 0,
		job_memory_limit: 0,
		peak_process_memory_used: 0,
		peak_job_memory_used: 0,
	};
}

/**
 * Opens a private kill-on-close Job Object and holds an exact process candidate.
 *
 * @since 0.4.0
 * @param process_id - PID of the suspended-by-protocol process host to own.
 * @returns A candidate that can be assigned only after IPC identity proof succeeds.
 */
export function open_windows_job_candidate(process_id: number): WindowsJobCandidate {
	const api = get_windows_api();
	const job = api.create_job_object(null, null);
	let closed = false;
	let assigned = false;

	if (job === null) {
		throw make_native_error("CreateJobObjectW", api.get_last_error);
	}

	const configured = api.set_information_job_object(
		job,
		job_object_extended_limit_information,
		make_limit_information(),
		koffi.sizeof(api.ExtendedLimitInformation),
	);

	if (!configured) {
		const cause = make_native_error("SetInformationJobObject", api.get_last_error);

		api.close_handle(job);

		throw cause;
	}

	const host_process = api.open_process(process_set_quota | process_terminate, 0, process_id);

	if (host_process === null) {
		const cause = make_native_error("OpenProcess", api.get_last_error);

		api.close_handle(job);

		throw cause;
	}

	const Close = () => {
		if (closed) {
			return;
		}

		closed = true;

		if (!assigned) {
			api.close_handle(host_process);
		}

		api.close_handle(job);
	};
	const Assign = (): WindowsJob => {
		if (closed || assigned) {
			throw new Error("The Windows Job candidate is no longer assignable");
		}

		try {
			if (!api.assign_process_to_job(job, host_process)) {
				throw make_native_error("AssignProcessToJobObject", api.get_last_error);
			}
		} catch (cause) {
			Close();

			throw cause;
		}

		assigned = true;
		api.close_handle(host_process);

		const Terminate = (exit_code: number) => {
			if (closed) {
				return;
			}

			if (!api.terminate_job_object(job, exit_code)) {
				throw make_native_error("TerminateJobObject", api.get_last_error);
			}
		};

		return { Close, Terminate };
	};

	return { Assign, Close };
}
