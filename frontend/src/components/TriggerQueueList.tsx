import { Link } from "react-router-dom";
import { Trigger } from "../../../bindings/Trigger";
import { TriggerQueue } from "../../../bindings/TriggerQueue";

interface Props {
	triggerQueue: TriggerQueue[],
	triggers: Trigger[],
}

export default function TriggerQueueList({ triggerQueue, triggers }: Props) {
	return <ul className="flex flex-col space-y-2">
		{triggerQueue.map(queue => {
			const trigger = triggers.filter( t => t.id === queue.trigger_id )[0];

			return <li key={ queue.id }>
				<Link className="flex space-x-3 p-2 bg-white/50 border-b border-b-slate-50 font-medium text-xs text-slate-600 items-center hover:text-purple-900" to={`/logs/queue/${queue.id}`}>
					<span>Created at {queue.created_at } for trigger { trigger && trigger.name }</span>
				</Link></li>
		})}
	</ul>
}
