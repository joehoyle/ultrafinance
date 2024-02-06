import { Await, defer, Link, useLoaderData, useRevalidator } from "react-router-dom";
import PageHeader from "../components/PageHeader";
import { useNavigate } from "react-router-dom";
import { getTriggerLog, getTriggerQueue, getTriggers, processTriggerQueue } from "../api";
import { Suspense, useState } from "react";
import LoadingList from "../components/LoadingList";
import TriggerQueueList from "../components/TriggerQueueList";
import TriggerLogList from "../components/TriggerLogList";
import { TriggerQueue } from "../../../bindings/TriggerQueue";
import { Trigger } from "../../../bindings/Trigger";
import { TriggerLog } from "../../../bindings/TriggerLog";
import Button from "../components/Button";

type Data = {
	triggerQueue: TriggerQueue[],
	triggers: Trigger[],
	log: TriggerLog[],
};

export async function loader() {
	return defer({
		triggerQueue: getTriggerQueue(),
		triggers: getTriggers(),
		log: getTriggerLog(),
	})
}

export default function Logs() {
	const { triggerQueue, triggers, log } = useLoaderData() as Data;

	const [processingQueue, setProcessingQueue] = useState(false);
	const revalidator = useRevalidator();

	async function onProcessQueue(e: React.MouseEvent<HTMLButtonElement, MouseEvent>) {
		e.preventDefault();
		setProcessingQueue(true);
		await processTriggerQueue();
		setProcessingQueue(false);
		revalidator.revalidate();
	}

	return <>
		<PageHeader>Logs</PageHeader>
		<main className=" flex-grow p-4">
			<Suspense>
				<Await resolve={Promise.all([triggerQueue, triggers])}>
					{([triggerQueue, triggers]) => <div className="mb-6">
						<h2 className="mb-4 flex">
							<span>
								Queued Jobs
							</span>
							<Button varient="secondary" className="ml-auto" onClick={onProcessQueue} isLoading={processingQueue} disabled={processingQueue}>Process Queue</Button>
						</h2>
						<TriggerQueueList triggerQueue={triggerQueue} triggers={triggers} />
					</div>}
				</Await>
			</Suspense>
			<Suspense fallback={<LoadingList number={15} />}>
				<Await resolve={Promise.all([log, triggers])}>
					{([log, triggers]) => <TriggerLogList log={log} triggers={triggers} />}
				</Await>
			</Suspense>
		</main>
	</>
}


