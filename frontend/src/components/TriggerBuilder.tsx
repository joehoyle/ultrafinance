import { useState } from "react";
import { Function } from "../../../bindings/Function";
import { Trigger } from "../../../bindings/Trigger";
import Input from "./forms/Input";
import FunctionParamField from "./FunctionParamField";

interface Props {
	functions: Function[],
	trigger?: Trigger,
}
export default function TriggerBuilder(props: Props) {
	const [selectedFunction, setSelectedFunction] = useState<Function | undefined>( props.functions.filter( f => f.id === props.trigger?.function_id )[0] );
	return <>
		<label>
			<div className="text-xs text-gray-500 mb-2">Trigger Name</div>
			<Input required defaultValue={ props.trigger?.name } name="name"  type="text" />
		</label>

		<p className="my-4 bg-purple-100 rounded-lg p-4">When
			<select
				required
				name="event"
				className="p-1 mx-2 border border-purple-400 rounded bg-purple-200 text-sm"
			>
				<option value="transaction_created">A transaction is created</option>
			</select> then run the function
			<select
				required
				onChange={e => {
					setSelectedFunction(props.functions[e.target.selectedIndex - 1])
				}}
				name="function_id"
				className="p-1 mx-2 border border-purple-400 rounded bg-purple-200 text-sm"
				defaultValue={ props.trigger?.function_id }
			>
				<option value="">- Select Function -</option>
				{props.functions.map(_function => (
					<option key={_function.id} value={_function.id}>{_function.name}</option>
				))}
			</select>
		</p>

		{selectedFunction && selectedFunction.params && Object.keys(selectedFunction.params).length > 0 &&
			<>
				<h4>{ selectedFunction.name } Configuration</h4>
				{Object.entries(selectedFunction.params).map(([paramId, param]) => (
					<FunctionParamField key={ paramId } param={ param } id={ paramId } value={ props.trigger?.params[ paramId ] } />
				))}
			</>
		}
	</>
}

