import { TriggerLog } from "../../../bindings/TriggerLog";
import { useNavigate } from "react-router-dom";
import { Trigger } from "../../../bindings/Trigger";

interface Props {
	log: TriggerLog[],
    triggers: Trigger[],
}
export default function TriggerLogList({ log, triggers }: Props) {
    const navigate = useNavigate();
    const sortedLog = log.sort( (a, b) => a.created_at > b.created_at ? -1 : 1 );
	return <ul className="flex flex-col space-y-2">
		<table className="text-sm w-full">
            <thead className="text-xs text-gray-500">
                <tr>
                    <td>Date</td>
                    <td>Trigger</td>
                    <td>Status</td>
                </tr>
            </thead>
            <tbody>
                {sortedLog.map(triggerLog => (
                    <tr key={ triggerLog.id } onClick={() => navigate(`/logs/${triggerLog.id}`)} className="border-b border-b-slate-50 hover:text-purple-800 hover:cursor-pointer">
                        <td className="py-3">{triggerLog.created_at}</td>
                        <td>{triggerLog.trigger_id}</td>
                        <td className={`text-xs ${triggerLog.status === 'error' && 'text-red-500'} ${triggerLog.status === 'completed' && 'text-green-500'}`}>{triggerLog.status}</td>
                    </tr>
                ))}
            </tbody>
        </table>
	</ul>
}
