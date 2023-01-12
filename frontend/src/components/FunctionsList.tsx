import { Link } from "react-router-dom";
import { Function } from "../../../bindings/Function";

interface Props {
	functions: Function[],
}

export default function FunctionsList({ functions }: Props) {
	return <ul className="flex flex-col space-y-2">
		{functions.map(_function => (
			<li key={ _function.id }>
				<Link className="flex space-x-3 p-2 bg-white/50 border-b border-b-slate-50 font-medium text-xs text-slate-600 items-center hover:text-purple-900" to={`/functions/${_function.id}`}>
					<span>{_function.name || '(untitled)'}</span>
				</Link></li>
		))}
	</ul>
}
