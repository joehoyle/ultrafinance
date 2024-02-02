import { Link } from "react-router-dom";
import { Trigger } from "../../../bindings/Trigger";

interface Props {
	triggers: Trigger[],
}

export default function TriggersList({ triggers }: Props) {
	return <ul className="flex flex-col space-y-2">
		{triggers.map(trigger => (
			<li key={trigger.id}>
				<Link className="flex space-x-3 p-2 bg-white/50 border-b border-b-slate-50 font-medium text-xs text-slate-600 items-center hover:text-purple-900" to={`/triggers/${trigger.id}`}>
					<span>{trigger.name || '(untitled)'} <span className="text-gray-400">on <code>{trigger.event}</code></span>.</span>
				</Link></li>
		))}
	</ul>
}
